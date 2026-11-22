//! Agent 觀察者(raychiutw/purrdr.host#30):主機服務常駐訂閱 herdr 的 Agent 狀態事件，不論 app 是否連線。
//!
//! 第一個用途是 Send When Done:每個 pane 一條佇列，Agent 從 working 變成 idle／done 時送出第一則
//! (經 `pane_guard` 比對 expected_session);`blocked` 之後的 idle 不算做完，session 不符時暫停並標記。
//! 佇列存在狀態資料夾，重啟後還原。另外定時讀 working Agent 的畫面指紋判斷可能卡住、看 blocked 多久提醒仍在等你
//! (raychiutw/purrdr.host#31),經推播配對送後台。觀察者不記活動紀錄與對話內容(佇列本身就是使用者要送的文字)。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::agent_push;
use crate::attention::{Alert, AttentionDetector, REMINDER_AFTER, STUCK_AFTER};
use crate::direct_push::DirectDelivery;
use crate::herdr_client::{HerdrClient, HerdrSubscription};
use crate::pane_guard::{self, GuardFailure};
use crate::pane_screen;
use crate::protocol::{PaneQueue, QueueChanged, QueuePause, QueuedPrompt, ServerMessage};
use crate::text_format::hex;
use crate::update_push::HttpPushTransport;

/// 每個 pane 最多排幾則。
pub const MAXIMUM_QUEUED_PROMPTS: usize = 20;
/// 每則最多幾 bytes(與 Pane 終端機單則輸入相同)。
pub const MAXIMUM_PROMPT_BYTES: usize = 64 * 1024;
/// 佇列檔名(狀態資料夾裡)。
const QUEUE_FILE: &str = "send-queue.json";

/// 觀察者的時間設定;測試以較短的值注入。
#[derive(Debug, Clone, Copy)]
pub struct ObserverOptions {
    /// herdr 訂閱斷掉後第一次重連前等多久，之後每次加倍。
    pub reconnect_initial: Duration,
    /// 重連等待的上限。
    pub reconnect_maximum: Duration,
    /// 多久讀一次 working Agent 的畫面指紋、檢查 blocked 多久(raychiutw/purrdr.host#31)。
    pub screen_check_interval: Duration,
    /// 畫面不變多久算可能卡住。
    pub stuck_after: Duration,
    /// 停在「需要你」多久再提醒。
    pub reminder_after: Duration,
}

impl Default for ObserverOptions {
    fn default() -> Self {
        ObserverOptions {
            reconnect_initial: Duration::from_secs(1),
            reconnect_maximum: Duration::from_secs(60),
            screen_check_interval: Duration::from_secs(30),
            stuck_after: STUCK_AFTER,
            reminder_after: REMINDER_AFTER,
        }
    }
}

/// 佇列操作失敗的原因。
#[derive(Debug)]
pub(crate) enum QueueFailure {
    Full,
    UnknownItem,
    /// 存不了佇列檔。
    Storage(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredQueues {
    #[serde(default)]
    panes: BTreeMap<String, StoredQueue>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredQueue {
    #[serde(default)]
    items: Vec<StoredPrompt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    paused: Option<QueuePause>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredPrompt {
    item_id: String,
    text: String,
    /// 送出前比對的 Agent session(必填：對象換了就不送)。
    expected_session: String,
}

/// Send When Done 佇列與變更通知;連線與觀察者 task 共用。
#[derive(Clone)]
pub(crate) struct AgentObserver {
    queues: Arc<Mutex<StoredQueues>>,
    file: PathBuf,
    changes: broadcast::Sender<ServerMessage>,
}

impl AgentObserver {
    /// 從狀態資料夾讀回佇列(檔案不在或壞了就從空的開始)。
    pub(crate) fn load(state_directory: &Path) -> Self {
        let file = state_directory.join(QUEUE_FILE);
        let queues = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let (changes, _) = broadcast::channel(64);
        AgentObserver {
            queues: Arc::new(Mutex::new(queues)),
            file,
            changes,
        }
    }

    /// 佇列變更的推送(queue.changed)。
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<ServerMessage> {
        self.changes.subscribe()
    }

    pub(crate) fn list(&self, pane_id: Option<&str>) -> Vec<PaneQueue> {
        let queues = self.queues.lock().unwrap();
        queues
            .panes
            .iter()
            .filter(|(pane, queue)| pane_id.is_none_or(|wanted| wanted == pane.as_str()) && !queue.items.is_empty())
            .map(|(pane, queue)| pane_queue(pane, queue))
            .collect()
    }

    pub(crate) fn add(&self, pane_id: &str, text: String, expected_session: String) -> Result<String, QueueFailure> {
        let item_id = new_item_id().map_err(|error| QueueFailure::Storage(error.to_string()))?;
        self.change(pane_id, None, |queue| {
            if queue.items.len() >= MAXIMUM_QUEUED_PROMPTS {
                return Err(QueueFailure::Full);
            }
            queue.items.push(StoredPrompt {
                item_id: item_id.clone(),
                text,
                expected_session,
            });
            // 不解除暫停：暫停是因為第一則的對象換了，加在後面不會改變這件事。
            Ok(())
        })?;
        Ok(item_id)
    }

    pub(crate) fn remove(&self, pane_id: &str, item_id: &str) -> Result<(), QueueFailure> {
        self.change(pane_id, None, |queue| {
            let position = queue.items.iter().position(|item| item.item_id == item_id).ok_or(QueueFailure::UnknownItem)?;
            queue.items.remove(position);
            queue.paused = None;
            Ok(())
        })
    }

    /// `item_ids` 必須恰好是目前佇列的所有項目。
    pub(crate) fn reorder(&self, pane_id: &str, item_ids: &[String]) -> Result<(), QueueFailure> {
        self.change(pane_id, None, |queue| {
            let mut current: Vec<&str> = queue.items.iter().map(|item| item.item_id.as_str()).collect();
            let mut wanted: Vec<&str> = item_ids.iter().map(String::as_str).collect();
            current.sort_unstable();
            wanted.sort_unstable();
            if current != wanted {
                return Err(QueueFailure::UnknownItem);
            }
            let mut items = std::mem::take(&mut queue.items);
            for item_id in item_ids {
                let position = items.iter().position(|item| &item.item_id == item_id).expect("已確認是同一組 id");
                queue.items.push(items.remove(position));
            }
            queue.paused = None;
            Ok(())
        })
    }

    /// 改一個 pane 的佇列：成功時存檔並推 queue.changed;失敗時不動。
    fn change(&self, pane_id: &str, sent_item_id: Option<String>, edit: impl FnOnce(&mut StoredQueue) -> Result<(), QueueFailure>) -> Result<(), QueueFailure> {
        let mut queues = self.queues.lock().unwrap();
        let mut queue = queues.panes.get(pane_id).cloned().unwrap_or_default();
        edit(&mut queue)?;
        let mut updated = queues.clone();
        if queue.items.is_empty() {
            updated.panes.remove(pane_id);
        } else {
            updated.panes.insert(pane_id.to_owned(), queue.clone());
        }
        save(&self.file, &updated).map_err(|error| QueueFailure::Storage(error.to_string()))?;
        *queues = updated;
        let _ = self.changes.send(ServerMessage::QueueChanged(QueueChanged {
            queue: pane_queue(pane_id, &queue),
            sent_item_id,
        }));
        Ok(())
    }

    /// 這個 pane 下一則要送的(暫停時沒有)。
    fn next(&self, pane_id: &str) -> Option<StoredPrompt> {
        let queues = self.queues.lock().unwrap();
        let queue = queues.panes.get(pane_id)?;
        if queue.paused.is_some() {
            return None;
        }
        queue.items.first().cloned()
    }
}

fn pane_queue(pane_id: &str, queue: &StoredQueue) -> PaneQueue {
    PaneQueue {
        pane_id: pane_id.to_owned(),
        items: queue
            .items
            .iter()
            .map(|item| QueuedPrompt {
                item_id: item.item_id.clone(),
                text: item.text.clone(),
                expected_session: item.expected_session.clone(),
            })
            .collect(),
        paused: queue.paused,
    }
}

/// 先寫暫存檔再改名，中途失敗不會留下半個檔案。
fn save(file: &Path, queues: &StoredQueues) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = file.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(queues).map_err(std::io::Error::other)?)?;
    std::fs::rename(&temporary, file)
}

fn new_item_id() -> Result<String, getrandom::Error> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes)?;
    Ok(hex(&bytes))
}

/// 啟動常駐 task:逐一 pane 訂閱 herdr 的 Agent 狀態事件(pane 增減時重新訂閱、斷線退避重連),依狀態轉換送出佇列，並定時檢查卡住與仍在等你。
/// `home` 是推播配對(`~/.purrdr/push-*`)所在的家目錄;None 時不推播。
/// `direct` 不為 None 時，轉換成 blocked／done 也由主機服務直接推播(raychiutw/purrdr.host#48):
/// 每個檢查週期(預設 30 秒)重新判斷能不能送(舊版外掛在就不送)並更新 lease,訂閱斷掉時收回 lease。
pub(crate) fn spawn(
    herdr: HerdrClient,
    observer: AgentObserver,
    options: ObserverOptions,
    home: Option<PathBuf>,
    direct: Option<DirectDelivery>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        // 上次異常結束可能留下還新鮮的 lease;先收回，確定能送了才重新宣告。
        if let Some(direct) = &direct {
            direct.release();
        }
        let mut statuses: HashMap<String, String> = HashMap::new();
        let mut attention = AttentionDetector::new(options.stuck_after, options.reminder_after);
        let mut delay = options.reconnect_initial;
        loop {
            if let Some((subscription, subscribed_panes)) = subscribe_to_panes(&herdr).await {
                delay = options.reconnect_initial;
                // pane 有增減時重新訂閱(不收回 lease、不等退避);herdr 結束訂閱才算斷線。
                let mut panes_changed = false;
                // 訂閱之後才讀 snapshot:中間的轉換會在訂閱裡，不會漏掉。
                if let Ok(snapshot) = herdr.request("session.snapshot", json!({})).await {
                    // 讀第一份 snapshot 到訂閱之間新開或關掉的 pane:訂閱沒涵蓋，重新訂閱。
                    panes_changed = snapshot_panes(&snapshot) != subscribed_panes;
                    let current = snapshot_statuses(&snapshot);
                    for (pane_id, status) in &current {
                        attention.status(pane_id, status, Instant::now());
                    }
                    // 斷線期間做完的 Agent:重連前還是 working、現在已 idle／done,照樣送出佇列。
                    let finished: Vec<String> = current
                        .iter()
                        .filter(|(pane_id, status)| statuses.get(*pane_id).map(String::as_str) == Some("working") && is_finished(status))
                        .map(|(pane_id, _)| pane_id.clone())
                        .collect();
                    statuses = current;
                    for pane_id in finished {
                        send_next(&herdr, &observer, &pane_id).await;
                    }
                }
                let mut sending = match &direct {
                    Some(direct) => direct.refresh().await,
                    None => false,
                };
                let (mut events, reader) = read_events(subscription);
                let mut checks = tokio::time::interval(options.screen_check_interval);
                while !panes_changed {
                    tokio::select! {
                        event = events.recv() => {
                            let Some(event) = event else { break };
                            if is_pane_added_or_removed(&event) {
                                panes_changed = true;
                            } else if let Some((pane_id, status)) = status_change(&event) {
                                attention.status(&pane_id, &status, Instant::now());
                                let previous = statuses.insert(pane_id.clone(), status.clone());
                                if previous.as_deref() == Some("working") && is_finished(&status) {
                                    send_next(&herdr, &observer, &pane_id).await;
                                }
                                // 只在「事件」上送、狀態有變才送;重連後的 snapshot 對帳不補送(斷線期間可能已由外掛接手)。
                                if sending
                                    && previous.as_deref() != Some(status.as_str())
                                    && let Some(kind) = direct_status(&status)
                                    && let Some(home) = home.as_deref()
                                {
                                    let (herdr, home, pane_id) = (herdr.clone(), home.to_owned(), pane_id.clone());
                                    tokio::spawn(async move { push(&herdr, &home, &pane_id, kind).await });
                                }
                            }
                        }
                        _ = checks.tick() => {
                            if let Some(direct) = &direct {
                                sending = direct.refresh().await;
                            }
                            check_attention(&herdr, &mut attention, home.as_deref()).await;
                        }
                    }
                }
                reader.abort();
                if panes_changed {
                    continue;
                }
                if let Some(direct) = &direct {
                    direct.release();
                }
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(options.reconnect_maximum);
        }
    })
}

/// 在另一個 task 讀訂閱(讀一行不能被中途取消),事件經 channel 交出;訂閱結束時 channel 關閉。
fn read_events(mut subscription: HerdrSubscription) -> (mpsc::UnboundedReceiver<Value>, JoinHandle<()>) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let reader = tokio::spawn(async move {
        while let Ok(Some(event)) = subscription.next_event().await {
            if sender.send(event).is_err() {
                return;
            }
        }
    });
    (receiver, reader)
}

/// working 的 Agent 讀畫面指紋判斷可能卡住;blocked 夠久的提醒仍在等你。
async fn check_attention(herdr: &HerdrClient, attention: &mut AttentionDetector, home: Option<&Path>) {
    let mut alerts = Vec::new();
    for pane_id in attention.working_panes() {
        if let Ok(fingerprint) = pane_screen::current_fingerprint(herdr, &pane_id).await
            && attention.screen(&pane_id, &fingerprint, Instant::now()) == Some(Alert::Stuck)
        {
            alerts.push((pane_id, "stuck"));
        }
    }
    alerts.extend(attention.reminders(Instant::now()).into_iter().map(|pane_id| (pane_id, "reminder")));
    let Some(home) = home else {
        return;
    };
    // 另開 task 送：後台慢的時候不擋住 Send When Done 的事件處理。
    for (pane_id, status) in alerts {
        let (herdr, home) = (herdr.clone(), home.to_owned());
        tokio::spawn(async move { push(&herdr, &home, &pane_id, status).await });
    }
}

/// 推播一則 Agent 狀態:workspace 名稱與 session(靜音比對)取自當下的 snapshot。
async fn push(herdr: &HerdrClient, home: &Path, pane_id: &str, status: &'static str) {
    let Ok(snapshot) = herdr.request("session.snapshot", json!({})).await else {
        return;
    };
    let session = pane_guard::session_of(&snapshot, pane_id);
    let Some(workspace) = workspace_label(&snapshot, pane_id) else {
        return;
    };
    let (home, pane_id) = (home.to_owned(), pane_id.to_owned());
    let _ = tokio::task::spawn_blocking(move || {
        agent_push::send(&home, &mut HttpPushTransport::default(), status, &workspace, &pane_id, session.as_deref());
    })
    .await;
}

/// pane 所在 workspace 的名稱(沒有名稱時用 workspace id,同主機外掛)。
fn workspace_label(snapshot: &Value, pane_id: &str) -> Option<String> {
    let snapshot = &snapshot["snapshot"];
    let workspace_id = snapshot["agents"].as_array()?.iter().find(|agent| agent["pane_id"] == pane_id)?["workspace_id"].as_str()?;
    let label = snapshot["workspaces"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|workspace| workspace["workspace_id"] == workspace_id)
        .and_then(|workspace| workspace["label"].as_str())
        .filter(|label| !label.is_empty());
    Some(label.unwrap_or(workspace_id).to_owned())
}

/// 由主機服務直接推播的狀態(與主機外掛相同:需要你、完成)。
fn direct_status(status: &str) -> Option<&'static str> {
    match status {
        "blocked" => Some("blocked"),
        "done" => Some("done"),
        _ => None,
    }
}

/// Agent 做完了(working 之後變成這些狀態才送佇列)。
fn is_finished(status: &str) -> bool {
    matches!(status, "idle" | "done")
}

fn snapshot_statuses(snapshot: &Value) -> HashMap<String, String> {
    snapshot["snapshot"]["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|agent| Some((agent["pane_id"].as_str()?.to_owned(), agent["agent_status"].as_str()?.to_owned())))
        .collect()
}

/// herdr 0.9.5 的 `pane.agent_status_changed` 只能逐一 pane 訂閱(必須帶 `pane_id`,raychiutw/purrdr.host#71):
/// 讀 snapshot 的 pane,訂閱每個 pane 的狀態變化，加上不必指定 pane 的 `pane.created`／`pane.closed`(pane 增減時重新訂閱)。
async fn subscribe_to_panes(herdr: &HerdrClient) -> Option<(HerdrSubscription, BTreeSet<String>)> {
    let snapshot = herdr.request("session.snapshot", json!({})).await.ok()?;
    let panes = snapshot_panes(&snapshot);
    let subscriptions: Vec<Value> = [json!({"type": "pane.created"}), json!({"type": "pane.closed"})]
        .into_iter()
        .chain(panes.iter().map(|pane_id| json!({"type": "pane.agent_status_changed", "pane_id": pane_id})))
        .collect();
    let subscription = herdr.subscribe(json!({ "subscriptions": subscriptions })).await.ok()?;
    Some((subscription, panes))
}

fn snapshot_panes(snapshot: &Value) -> BTreeSet<String> {
    snapshot["snapshot"]["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pane| Some(pane["pane_id"].as_str()?.to_owned()))
        .collect()
}

/// 訂閱裡除了 Agent 狀態只有 pane 新增／關閉;herdr 的事件名稱有 `pane.created` 與 `pane_created` 兩種寫法。
fn is_pane_added_or_removed(event: &Value) -> bool {
    matches!(
        event["event"].as_str().map(|name| name.replace('.', "_")).as_deref(),
        Some("pane_created" | "pane_closed")
    )
}

fn status_change(event: &Value) -> Option<(String, String)> {
    if event["event"] != "pane.agent_status_changed" {
        return None;
    }
    let data = &event["data"];
    Some((data["pane_id"].as_str()?.to_owned(), data["agent_status"].as_str()?.to_owned()))
}

/// 送出佇列第一則：比對通過才送，送出後從佇列移除;session 不符時暫停並標記;herdr 失敗時留著等下次。
async fn send_next(herdr: &HerdrClient, observer: &AgentObserver, pane_id: &str) {
    let Some(item) = observer.next(pane_id) else {
        return;
    };
    match pane_guard::check(herdr, pane_id, Some(&item.expected_session), None).await {
        Ok(()) => {}
        Err(GuardFailure::Changed(_)) => {
            let _ = observer.change(pane_id, None, |queue| {
                queue.paused = Some(QueuePause::SessionChanged);
                Ok(())
            });
            return;
        }
        Err(GuardFailure::Herdr { .. }) => return,
    }
    if herdr.request("agent.prompt", json!({"target": pane_id, "text": item.text})).await.is_ok() {
        let sent = item.item_id.clone();
        let _ = observer.change(pane_id, Some(sent), |queue| {
            queue.items.retain(|queued| queued.item_id != item.item_id);
            Ok(())
        });
    }
}
