//! 主機服務直接送 blocked／done 推播(raychiutw/purrdr.host#48,0.12.0 起):
//! Agent 觀察者在狀態轉換成 blocked／done 時用推播配對(`~/.purrdr/push-*`)送後台，並維護 lease
//! (`~/.purrdr/host-service/push-sender`)讓主機外掛 0.2.0 讓位;已裝舊版外掛 0.1.0(沒有讓位邏輯)時不接手，避免同一則通知兩次。
//! `push.install` 在主機服務能直接送時不安裝外掛，回覆帶 `direct_push`。
//!
//! 假的 herdr socket(`FakeHerdr`)給事件與 snapshot,假的 herdr CLI 就是這支測試執行檔本身(`fake-herdr <資料夾>` 參數):
//! 資料夾裡 `version` 的內容就是已安裝外掛的版本(沒有檔案代表沒裝),`list-fails` 存在時 `plugin list` 失敗，
//! 每次呼叫的參數記在 `calls`。所以不用 libtest(`harness = false`)。

mod install_support;
mod support;

use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use install_support::push_backend::FakePushBackend;
use purrdr_host_service::agent_observer::ObserverOptions;
use purrdr_host_service::herdr_cli::HerdrCli;
use serde_json::{Value, json};
use support::{FakeHerdr, TestClient, TestHostService, recorded, start_service_with_observer_and_cli};
use tempfile::TempDir;

const FAKE_HERDR: &str = "fake-herdr";
const PHONE_KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const WATCH_KEY: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const INSTALL_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
/// session.snapshot.ndjson 裡 w1:pA 的 session。
const PA_SESSION: &str = "00000000-0000-0000-0000-000000000002";

// MARK: - 假的 herdr CLI

fn fake_herdr(arguments: &[String]) -> i32 {
    let directory = PathBuf::from(&arguments[0]);
    let herdr_arguments = &arguments[1..];
    let mut calls = std::fs::OpenOptions::new().create(true).append(true).open(directory.join("calls")).unwrap();
    writeln!(calls, "{}", herdr_arguments.join(" ")).unwrap();
    let plugin = herdr_arguments.iter().position(|argument| argument == "plugin").unwrap();
    match herdr_arguments[plugin..].join(" ").as_str() {
        "plugin list --plugin purrdr.push --json" => {
            if directory.join("list-fails").exists() {
                eprintln!("herdr server is not running");
                return 1;
            }
            let plugins = match std::fs::read_to_string(directory.join("version")) {
                Ok(version) => json!([{"plugin_id": "purrdr.push", "name": "Purrdr Push", "version": version.trim(), "enabled": true}]),
                Err(_) => json!([]),
            };
            println!("{}", json!({"id": "cli:plugin", "result": {"plugins": plugins, "type": "plugin_list"}}));
            0
        }
        "plugin install raychiutw/purrdr.host/host-plugin --yes" => {
            std::fs::write(directory.join("version"), "0.2.0").unwrap();
            0
        }
        "plugin uninstall purrdr.push" => {
            let _ = std::fs::remove_file(directory.join("version"));
            0
        }
        other => {
            eprintln!("unexpected: {other}");
            2
        }
    }
}

// MARK: - 測試環境

fn options() -> ObserverOptions {
    ObserverOptions {
        reconnect_initial: Duration::from_millis(20),
        reconnect_maximum: Duration::from_millis(80),
        screen_check_interval: Duration::from_millis(20),
        // 不讓 stuck／reminder 的推播干擾這裡的計數。
        stuck_after: Duration::from_secs(3600),
        reminder_after: Duration::from_secs(3600),
    }
}

struct Harness {
    herdr: FakeHerdr,
    fake: TempDir,
    home: TempDir,
    backend: FakePushBackend,
    service: Option<TestHostService>,
}

impl Harness {
    /// `version`:herdr 回報已安裝的外掛版本(None 是沒裝)。
    async fn start(version: Option<&str>) -> Harness {
        let herdr = FakeHerdr::start(vec![
            ("ping", recorded("ping.ndjson")),
            ("session.snapshot", recorded("session.snapshot.ndjson")),
            ("events.subscribe", recorded("events.subscribe.started.ndjson")),
            ("pane.read", recorded("pane.read.question.ndjson")),
        ])
        .await;
        let fake = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let backend = FakePushBackend::start();
        if let Some(version) = version {
            std::fs::write(fake.path().join("version"), version).unwrap();
        }
        pair(home.path(), "phone", &backend.endpoint(), PHONE_KEY, &[]);
        pair(home.path(), "watch", &backend.endpoint(), WATCH_KEY, &[PA_SESSION]);
        let cli = HerdrCli::at_path(
            std::env::current_exe().unwrap(),
            vec![FAKE_HERDR.to_owned(), fake.path().to_string_lossy().into_owned()],
        );
        let service = start_service_with_observer_and_cli(herdr.socket.clone(), cli, None, Some(home.path().to_owned()), options()).await;
        herdr.wait_until(|herdr| herdr.open_subscriptions() == 1).await;
        Harness {
            herdr,
            fake,
            home,
            backend,
            service: Some(service),
        }
    }

    fn lease_path(&self) -> PathBuf {
        self.home.path().join(".purrdr/host-service/push-sender")
    }

    /// lease 檔的 `名稱=值`(沒有檔案是 None)。
    fn lease(&self) -> Option<Vec<(String, String)>> {
        let text = std::fs::read_to_string(self.lease_path()).ok()?;
        Some(
            text.lines()
                .filter_map(|line| line.split_once('='))
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
        )
    }

    fn lease_value(&self, name: &str) -> Option<String> {
        self.lease()?.into_iter().find(|(key, _)| key == name).map(|(_, value)| value)
    }

    async fn wait_for_lease(&self) {
        wait(|| self.lease().is_some(), "等不到 lease").await;
    }

    /// herdr 外掛清單被問過幾次(觀察者每個週期問一次)。
    fn list_calls(&self) -> usize {
        self.calls().iter().filter(|call| call.contains("plugin list")).count()
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.fake.path().join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// 等觀察者至少又判斷了兩輪(之後才能說「沒有」發生什麼)。
    async fn settle(&self) {
        let before = self.list_calls();
        wait(|| self.list_calls() >= before + 3, "觀察者沒有繼續檢查外掛").await;
    }

    fn set_version(&self, version: Option<&str>) {
        let file = self.fake.path().join("version");
        match version {
            Some(version) => std::fs::write(file, version).unwrap(),
            None => std::fs::remove_file(file).unwrap(),
        }
    }

    fn event(&self, pane: &str, status: &str) {
        self.herdr.push_event(
            json!({"event": "pane.agent_status_changed", "data": {"agent": "claude", "agent_status": status, "pane_id": pane, "workspace_id": "w1"}}),
        );
    }

    fn bodies(&self, status: &str) -> Vec<Value> {
        self.backend.bodies().into_iter().filter(|body| body["status"] == status).collect()
    }

    fn sent(&self, key: &str, status: &str, pane: &str) -> usize {
        self.bodies(status).iter().filter(|body| body["key"] == key && body["pane"] == pane).count()
    }

    async fn client(&self) -> TestClient {
        let mut client = TestClient::connect(self.service.as_ref().unwrap()).await;
        client.request(json!({"type": "hello", "request_id": "h1", "protocol_version": 1})).await;
        client
    }
}

async fn wait(condition: impl Fn() -> bool, message: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(tokio::time::Instant::now() < deadline, "{message}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn pair(home: &Path, device: &str, endpoint: &str, key: &str, muted: &[&str]) {
    let directory = home.join(".purrdr").join(format!("push-{device}"));
    std::fs::create_dir_all(&directory).unwrap();
    let muted: String = muted.iter().map(|session| format!("muted={session}\n")).collect();
    std::fs::write(directory.join("pairing"), format!("purrdr-push 1\nendpoint={endpoint}\nkey={key}\n{muted}")).unwrap();
}

/// 主機服務寫進 lease 的 socket 路徑:資料夾正規化(Windows 的 named pipe 名稱照原樣)。
fn canonical_socket(path: &str) -> String {
    if cfg!(windows) {
        return path.to_owned();
    }
    let path = Path::new(path);
    let directory = std::fs::canonicalize(path.parent().unwrap()).unwrap();
    directory.join(path.file_name().unwrap()).to_string_lossy().into_owned()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

// MARK: - 直接送 blocked／done

async fn blocked_and_done_are_sent_directly_once_per_transition() {
    let harness = Harness::start(None).await;
    harness.wait_for_lease().await;

    harness.event("w1:pB", "blocked");
    wait(|| harness.sent(PHONE_KEY, "blocked", "w1:pB") == 1, "等不到 blocked").await;
    harness.event("w1:pA", "done");
    wait(|| harness.sent(PHONE_KEY, "done", "w1:pA") == 1, "等不到 done").await;
    // 同一個狀態又來一次:不重送。
    harness.event("w1:pB", "blocked");
    // 不是 blocked／done 的狀態不送。
    harness.event("w1:p6", "working");
    harness.event("w1:p6", "idle");
    harness.settle().await;

    let bodies = harness.backend.bodies();
    // 只帶密鑰、workspace 名稱、狀態、pane id 與 tab(後台多一個欄位就回 400)。
    // recorded snapshot 的 Tab 名稱是 herdr 預設的順序號(`3`),不算 Tab label → tab 用 Agent 名稱。
    assert!(
        bodies.contains(&json!({"key": PHONE_KEY, "workspace": "garden-journal", "status": "blocked", "pane": "w1:pB", "tab": "claude"})),
        "{bodies:?}"
    );
    assert_eq!(harness.sent(PHONE_KEY, "blocked", "w1:pB"), 1, "{bodies:?}");
    assert_eq!(harness.bodies("working").len() + harness.bodies("idle").len(), 0, "{bodies:?}");
    // watch 靜音了 w1:pA 的聊天,w1:pB 照送。
    assert_eq!(harness.sent(WATCH_KEY, "done", "w1:pA"), 0, "{bodies:?}");
    assert_eq!(harness.sent(WATCH_KEY, "blocked", "w1:pB"), 1, "{bodies:?}");
    // 回到 working 之後再 blocked 是新的轉換，要送。
    harness.event("w1:pB", "working");
    harness.event("w1:pB", "blocked");
    wait(|| harness.sent(PHONE_KEY, "blocked", "w1:pB") == 2, "再次 blocked 沒有送").await;
}

async fn a_revoked_pairing_is_removed_when_sending_blocked() {
    let harness = Harness::start(None).await;
    harness.backend.respond_to_key(WATCH_KEY, 410);
    harness.wait_for_lease().await;

    harness.event("w1:pB", "blocked");

    wait(|| harness.sent(PHONE_KEY, "blocked", "w1:pB") == 1, "等不到 blocked").await;
    wait(|| !harness.home.path().join(".purrdr/push-watch").exists(), "被撤銷的配對沒有刪掉").await;
    assert!(harness.home.path().join(".purrdr/push-phone/pairing").exists());
}

/// recorded snapshot 多一個 w1:pC(複製 w1:pB 的 pane 與 Agent),回傳 (snapshot, 新 pane)。
fn snapshot_with_pane_c() -> (Value, Value) {
    let mut snapshot = recorded("session.snapshot.ndjson");
    let panes = snapshot["result"]["snapshot"]["panes"].as_array_mut().unwrap();
    let mut opened = panes.iter().find(|pane| pane["pane_id"] == "w1:pB").unwrap().clone();
    opened["pane_id"] = json!("w1:pC");
    panes.push(opened.clone());
    let agents = snapshot["result"]["snapshot"]["agents"].as_array_mut().unwrap();
    let mut agent = agents.iter().find(|agent| agent["pane_id"] == "w1:pB").unwrap().clone();
    agent["pane_id"] = json!("w1:pC");
    agents.push(agent);
    (snapshot, opened)
}

async fn wait_for_subscription_to_pane_c(herdr: &FakeHerdr) {
    herdr
        .wait_until(|herdr| {
            herdr
                .last_subscription()
                .contains(&("pane.agent_status_changed".to_owned(), Some("w1:pC".to_owned())))
        })
        .await;
    herdr.wait_until(|herdr| herdr.open_subscriptions() == 1).await;
}

/// herdr 只能逐一 pane 訂閱 Agent 狀態(raychiutw/purrdr.host#71):新開的 pane 要重新訂閱才收得到它的事件。
async fn a_pane_opened_later_is_observed_too() {
    let harness = Harness::start(None).await;
    harness.wait_for_lease().await;
    let (snapshot, opened) = snapshot_with_pane_c();
    harness.herdr.set_reply("session.snapshot", snapshot);
    // 重新訂閱期間 lease 不能消失(消失的那一刻外掛會照送，造成重複)。
    let watching = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let watcher = {
        let (watching, lease) = (watching.clone(), harness.lease_path());
        std::thread::spawn(move || {
            let mut missing = false;
            while watching.load(std::sync::atomic::Ordering::Relaxed) {
                missing |= !lease.exists();
                std::thread::sleep(Duration::from_micros(200));
            }
            missing
        })
    };

    harness
        .herdr
        .push_event(json!({"event": "pane_created", "data": {"pane": opened, "workspace_id": "w1"}}));
    wait_for_subscription_to_pane_c(&harness.herdr).await;
    harness.event("w1:pC", "blocked");

    wait(|| harness.sent(PHONE_KEY, "blocked", "w1:pC") == 1, "新開的 pane 轉成 blocked 沒有送").await;
    watching.store(false, std::sync::atomic::Ordering::Relaxed);
    assert!(!watcher.join().unwrap(), "重新訂閱期間 lease 不能消失");
}

/// 讀 snapshot 決定要訂閱哪些 pane 之後、訂閱生效之前開的 pane:訂閱後的 snapshot 多了它，要重新訂閱。
async fn a_pane_opened_while_subscribing_is_observed_too() {
    let harness = Harness::start(None).await;
    harness.wait_for_lease().await;
    let (snapshot, _) = snapshot_with_pane_c();
    harness.herdr.set_reply("session.snapshot", snapshot);
    harness.herdr.reply_once("session.snapshot", recorded("session.snapshot.ndjson"));

    // herdr 結束訂閱 → 重連:第一份 snapshot 還沒有 w1:pC,訂閱之後的 snapshot 有。
    harness.herdr.end_subscriptions(None);
    wait_for_subscription_to_pane_c(&harness.herdr).await;
    harness.event("w1:pC", "blocked");

    wait(|| harness.sent(PHONE_KEY, "blocked", "w1:pC") == 1, "訂閱期間新開的 pane 轉成 blocked 沒有送").await;
}

/// herdr 沒為新 pane 發事件(例如新 workspace 的第一個 pane)時，檢查週期比對 snapshot 也會發現並訂閱。
async fn a_pane_without_a_created_event_is_found_by_the_periodic_check() {
    let harness = Harness::start(None).await;
    harness.wait_for_lease().await;
    let (snapshot, _) = snapshot_with_pane_c();
    harness.herdr.set_reply("session.snapshot", snapshot);

    wait_for_subscription_to_pane_c(&harness.herdr).await;
    harness.event("w1:pC", "blocked");

    wait(|| harness.sent(PHONE_KEY, "blocked", "w1:pC") == 1, "檢查週期找到的 pane 轉成 blocked 沒有送").await;
}

/// 重新訂閱期間 lease 一直由主機服務持有(外掛不送):這段期間變成 blocked 的，對帳時由主機服務補送。
async fn a_pane_that_became_blocked_while_resubscribing_is_still_sent() {
    let harness = Harness::start(None).await;
    harness.wait_for_lease().await;
    let (mut snapshot, opened) = snapshot_with_pane_c();
    let agents = snapshot["result"]["snapshot"]["agents"].as_array_mut().unwrap();
    agents.iter_mut().find(|agent| agent["pane_id"] == "w1:pB").unwrap()["agent_status"] = json!("blocked");
    harness.herdr.set_reply("session.snapshot", snapshot);

    harness
        .herdr
        .push_event(json!({"event": "pane_created", "data": {"pane": opened, "workspace_id": "w1"}}));

    wait(|| harness.sent(PHONE_KEY, "blocked", "w1:pB") == 1, "重新訂閱期間變成 blocked 沒有補送").await;
    harness.settle().await;
    assert_eq!(harness.sent(PHONE_KEY, "blocked", "w1:pB"), 1, "只補送一次");
}

/// 重新訂閱失敗(例如 pane 在讀 snapshot 與訂閱之間關掉，herdr 回 pane_not_found)時收回 lease,讓外掛接手。
async fn a_failed_resubscription_gives_the_lease_back() {
    let harness = Harness::start(None).await;
    harness.wait_for_lease().await;
    harness.herdr.set_reply("events.subscribe", recorded("error.pane-not-found.ndjson"));
    let (_, opened) = snapshot_with_pane_c();

    harness
        .herdr
        .push_event(json!({"event": "pane_created", "data": {"pane": opened, "workspace_id": "w1"}}));

    wait(|| harness.lease().is_none(), "重新訂閱失敗時沒有收回 lease").await;
}

// MARK: - 舊外掛不共存

async fn a_legacy_plugin_keeps_the_host_service_from_sending_and_from_taking_the_lease() {
    let harness = Harness::start(Some("0.1.0")).await;

    harness.event("w1:pB", "blocked");
    harness.event("w1:pA", "done");
    harness.settle().await;

    assert!(
        harness.bodies("blocked").is_empty() && harness.bodies("done").is_empty(),
        "{:?}",
        harness.backend.bodies()
    );
    assert_eq!(harness.lease(), None, "舊外掛不會讓位，不能宣告接手");
}

async fn only_plugin_versions_that_yield_allow_direct_sending() {
    // (版本, 主機服務是否接手):數字比對，0.10.0 比 0.2.0 新;看不懂的版本當作舊版。
    let cases = [
        ("0.1.0", false),
        ("0.1.9", false),
        ("0.0.1", false),
        ("0.2.0", true),
        ("0.2.1", true),
        ("0.10.0", true),
        ("1.0.0", true),
        ("garbage", false),
        ("", false),
        ("0.2", false),
    ];
    for (version, takes_over) in cases {
        let harness = Harness::start(Some(version)).await;
        if takes_over {
            harness.wait_for_lease().await;
        } else {
            harness.settle().await;
            assert_eq!(harness.lease(), None, "版本 {version:?} 不該接手");
        }
    }
}

async fn host_service_takes_over_when_the_plugin_is_upgraded_and_hands_back_when_downgraded() {
    let harness = Harness::start(Some("0.1.0")).await;
    harness.settle().await;
    assert_eq!(harness.lease(), None);

    harness.set_version(Some("0.2.0"));
    harness.wait_for_lease().await;
    harness.event("w1:pB", "blocked");
    wait(|| harness.sent(PHONE_KEY, "blocked", "w1:pB") == 1, "升級後沒有接手").await;

    harness.set_version(Some("0.1.0"));
    wait(|| harness.lease().is_none(), "舊外掛回來時沒有交還 lease").await;
    harness.event("w1:pB", "working");
    harness.event("w1:pB", "blocked");
    harness.settle().await;
    assert_eq!(harness.sent(PHONE_KEY, "blocked", "w1:pB"), 1, "交還之後主機服務不能再送");
}

async fn when_herdr_cannot_list_plugins_the_host_service_does_not_send() {
    let harness = Harness::start(None).await;
    harness.wait_for_lease().await;
    std::fs::write(harness.fake.path().join("list-fails"), b"").unwrap();

    wait(|| harness.lease().is_none(), "查不到外掛時沒有收回 lease").await;
    harness.event("w1:pB", "blocked");
    harness.settle().await;

    assert!(harness.bodies("blocked").is_empty(), "{:?}", harness.backend.bodies());
}

// MARK: - lease

async fn lease_records_the_observed_socket_and_is_refreshed() {
    let harness = Harness::start(None).await;
    harness.wait_for_lease().await;

    assert_eq!(harness.lease_value("socket"), Some(canonical_socket(harness.herdr.socket.path())));
    let first: u64 = harness.lease_value("updated").unwrap().parse().unwrap();
    assert!(first.abs_diff(unix_now()) <= 5, "updated 應是現在的 Unix 秒:{first}");
    wait(
        || {
            harness
                .lease_value("updated")
                .and_then(|value| value.parse::<u64>().ok())
                .is_some_and(|updated| updated > first)
        },
        "lease 沒有定期更新",
    )
    .await;
    assert_eq!(harness.lease_value("socket"), Some(canonical_socket(harness.herdr.socket.path())));
}

async fn lease_is_removed_when_the_herdr_subscription_ends() {
    let harness = Harness::start(None).await;
    harness.wait_for_lease().await;

    harness.herdr.end_subscriptions(None);

    wait(|| harness.lease().is_none(), "訂閱結束時沒有刪 lease").await;
    // 重連之後又接手。
    harness.wait_for_lease().await;
}

async fn lease_is_removed_when_the_service_stops() {
    let mut harness = Harness::start(None).await;
    harness.wait_for_lease().await;

    drop(harness.service.take());

    assert_eq!(harness.lease(), None);
}

// MARK: - push.install 不裝外掛

async fn install_does_not_install_the_plugin_when_the_host_service_sends_directly() {
    let harness = Harness::start(None).await;
    harness.wait_for_lease().await;
    let mut client = harness.client().await;

    let reply = client
        .request(json!({
            "type": "push.install", "request_id": "i1", "device_id": "tablet",
            "pairing": {"endpoint": "https://push.example.com/v1/push", "key": INSTALL_KEY},
        }))
        .await;

    assert_eq!(
        reply,
        json!({"type": "push.install.result", "request_id": "i1", "plugin_installed": false, "paired": true, "other_devices": 2, "direct_push": true})
    );
    assert!(harness.home.path().join(".purrdr/push-tablet/pairing").is_file());
    assert!(!harness.calls().iter().any(|call| call.contains("plugin install")), "{:?}", harness.calls());
    assert!(!harness.home.path().join(".purrdr/push-plugin").exists(), "沒有裝外掛就不該留下安裝紀錄");

    // 關閉:沒有 Purrdr 裝的外掛，不會執行 uninstall。
    let removed = client.request(json!({"type": "push.remove", "request_id": "r1", "device_id": "tablet"})).await;
    assert_eq!(removed["direct_push"], true, "{removed}");
    assert_eq!(removed["plugin_installed"], false, "{removed}");
    assert!(!harness.calls().iter().any(|call| call.contains("uninstall")), "{:?}", harness.calls());
}

async fn status_reports_direct_push_for_each_plugin_state() {
    // (已裝外掛的版本, plugin_installed, direct_push)
    let cases = [
        (None, false, true),
        (Some("0.1.0"), true, false),
        (Some("0.2.0"), true, true),
        (Some("garbage"), true, false),
    ];
    for (version, installed, direct) in cases {
        let harness = Harness::start(version).await;
        let mut client = harness.client().await;

        let reply = client.request(json!({"type": "push.status", "request_id": "s1", "device_id": "tablet"})).await;

        assert_eq!(reply["plugin_installed"], installed, "{version:?} {reply}");
        assert_eq!(reply["direct_push"], direct, "{version:?} {reply}");
    }
}

async fn install_with_a_legacy_plugin_keeps_the_old_behaviour() {
    let harness = Harness::start(Some("0.1.0")).await;
    let mut client = harness.client().await;

    let reply = client
        .request(json!({
            "type": "push.install", "request_id": "i1", "device_id": "tablet",
            "pairing": {"endpoint": "https://push.example.com/v1/push", "key": INSTALL_KEY},
        }))
        .await;

    assert_eq!(reply["plugin_installed"], true, "{reply}");
    assert_eq!(reply["direct_push"], false, "{reply}");
    assert!(!harness.calls().iter().any(|call| call.contains("plugin install")), "已裝的外掛不重裝");
}

async fn update_muted_reply_has_no_direct_push() {
    let harness = Harness::start(None).await;
    pair(harness.home.path(), "tablet", "https://push.example.com/v1/push", INSTALL_KEY, &[]);
    let mut client = harness.client().await;

    let reply = client
        .request(json!({"type": "push.update_muted", "request_id": "m1", "device_id": "tablet", "muted": ["x"]}))
        .await;

    assert_eq!(reply["type"], "push.update_muted.result", "{reply}");
    assert!(reply.get("direct_push").is_none(), "{reply}");
}

// MARK: - 執行

// MARK: - tab 欄位(raychiutw/purrdr.host#74)

/// recorded snapshot 裡 w1:pB 所在 Tab 的名稱與 Agent 名稱改成指定的值(`None` 是移除欄位)。
fn snapshot_with_tab(label: Option<&str>, agent: Option<&str>) -> Value {
    let mut snapshot = recorded("session.snapshot.ndjson");
    let body = &mut snapshot["result"]["snapshot"];
    let tab = body["tabs"].as_array_mut().unwrap().iter_mut().find(|tab| tab["tab_id"] == "w1:tB").unwrap();
    match label {
        Some(label) => tab["label"] = json!(label),
        None => {
            tab.as_object_mut().unwrap().remove("label");
        }
    }
    let pane = body["agents"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|agent| agent["pane_id"] == "w1:pB")
        .unwrap();
    match agent {
        Some(agent) => pane["agent"] = json!(agent),
        None => {
            pane.as_object_mut().unwrap().remove("agent");
        }
    }
    snapshot
}

/// 這個 snapshot 下 w1:pB 轉成 blocked 時送出的本文(收到第一則)。
async fn blocked_body_with(snapshot: Value) -> Value {
    let harness = Harness::start(None).await;
    harness.wait_for_lease().await;
    harness.herdr.set_reply("session.snapshot", snapshot);
    harness.event("w1:pB", "blocked");
    wait(|| harness.sent(PHONE_KEY, "blocked", "w1:pB") == 1, "等不到 blocked").await;
    harness.bodies("blocked").into_iter().find(|body| body["key"] == PHONE_KEY).unwrap()
}

/// 有 Tab label:`tab` 用它;終端機標題(對話內容)不會跟著送出。
async fn the_tab_label_is_sent_when_the_tab_has_one() {
    let body = blocked_body_with(snapshot_with_tab(Some("後台 API"), Some("claude"))).await;
    assert_eq!(
        body,
        json!({"key": PHONE_KEY, "workspace": "garden-journal", "status": "blocked", "pane": "w1:pB", "tab": "後台 API"})
    );
}

/// 沒有 Tab label(沒有欄位、空字串,或 herdr 預設的順序號):用該 Pane 的 Agent 名稱。
async fn without_a_tab_label_the_agent_name_is_sent() {
    for label in [None, Some(""), Some("3")] {
        let body = blocked_body_with(snapshot_with_tab(label, Some("codex"))).await;
        assert_eq!(body["tab"], "codex", "{label:?} {body}");
    }
}

/// 150 個字元的 Agent 名稱。
const LONG_AGENT: &str = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";

/// 後台的 `tab` 最長 100 字元、全空白視同沒帶:前後空白去掉、以字元計截斷(不切壞 UTF-8)、空白 label 改用 Agent 名稱。
async fn the_tab_is_trimmed_and_cut_to_100_characters() {
    let cases = [
        (Some("天".repeat(150)), Some("claude"), Some("天".repeat(100))),
        (Some("天".repeat(100)), Some("claude"), Some("天".repeat(100))),
        (Some("é🙂".repeat(80)), Some("claude"), Some("é🙂".repeat(50))),
        (Some("  後台 API \t".to_owned()), Some("claude"), Some("後台 API".to_owned())),
        (Some("   ".to_owned()), Some("claude"), Some("claude".to_owned())),
        (None, Some("  codex "), Some("codex".to_owned())),
        (None, Some(LONG_AGENT), Some("x".repeat(100))),
        (Some(format!("{} tail", "a".repeat(99))), Some("claude"), Some("a".repeat(99))),
        (None, Some("   "), None),
    ];
    for (label, agent, expected) in cases {
        let body = blocked_body_with(snapshot_with_tab(label.as_deref(), agent)).await;
        assert_eq!(body.get("tab").and_then(Value::as_str).map(str::to_owned), expected, "{label:?} {agent:?}");
    }
}

/// Tab label 與 Agent 名稱都沒有:省略 `tab`(其餘欄位照送)。
async fn the_tab_is_omitted_without_a_label_and_an_agent() {
    for agent in [None, Some("")] {
        let body = blocked_body_with(snapshot_with_tab(None, agent)).await;
        assert_eq!(
            body,
            json!({"key": PHONE_KEY, "workspace": "garden-journal", "status": "blocked", "pane": "w1:pB"}),
            "{agent:?}"
        );
    }
}

type TestCase = (&'static str, fn() -> Pin<Box<dyn Future<Output = ()>>>);

fn tests() -> Vec<TestCase> {
    vec![
        ("blocked_and_done_are_sent_directly_once_per_transition", || {
            Box::pin(blocked_and_done_are_sent_directly_once_per_transition())
        }),
        ("the_tab_label_is_sent_when_the_tab_has_one", || {
            Box::pin(the_tab_label_is_sent_when_the_tab_has_one())
        }),
        ("without_a_tab_label_the_agent_name_is_sent", || {
            Box::pin(without_a_tab_label_the_agent_name_is_sent())
        }),
        ("the_tab_is_trimmed_and_cut_to_100_characters", || {
            Box::pin(the_tab_is_trimmed_and_cut_to_100_characters())
        }),
        ("the_tab_is_omitted_without_a_label_and_an_agent", || {
            Box::pin(the_tab_is_omitted_without_a_label_and_an_agent())
        }),
        ("a_revoked_pairing_is_removed_when_sending_blocked", || {
            Box::pin(a_revoked_pairing_is_removed_when_sending_blocked())
        }),
        ("a_pane_opened_later_is_observed_too", || Box::pin(a_pane_opened_later_is_observed_too())),
        ("a_pane_opened_while_subscribing_is_observed_too", || {
            Box::pin(a_pane_opened_while_subscribing_is_observed_too())
        }),
        ("a_pane_without_a_created_event_is_found_by_the_periodic_check", || {
            Box::pin(a_pane_without_a_created_event_is_found_by_the_periodic_check())
        }),
        ("a_pane_that_became_blocked_while_resubscribing_is_still_sent", || {
            Box::pin(a_pane_that_became_blocked_while_resubscribing_is_still_sent())
        }),
        ("a_failed_resubscription_gives_the_lease_back", || {
            Box::pin(a_failed_resubscription_gives_the_lease_back())
        }),
        ("a_legacy_plugin_keeps_the_host_service_from_sending_and_from_taking_the_lease", || {
            Box::pin(a_legacy_plugin_keeps_the_host_service_from_sending_and_from_taking_the_lease())
        }),
        ("only_plugin_versions_that_yield_allow_direct_sending", || {
            Box::pin(only_plugin_versions_that_yield_allow_direct_sending())
        }),
        ("host_service_takes_over_when_the_plugin_is_upgraded_and_hands_back_when_downgraded", || {
            Box::pin(host_service_takes_over_when_the_plugin_is_upgraded_and_hands_back_when_downgraded())
        }),
        ("when_herdr_cannot_list_plugins_the_host_service_does_not_send", || {
            Box::pin(when_herdr_cannot_list_plugins_the_host_service_does_not_send())
        }),
        ("lease_records_the_observed_socket_and_is_refreshed", || {
            Box::pin(lease_records_the_observed_socket_and_is_refreshed())
        }),
        ("lease_is_removed_when_the_herdr_subscription_ends", || {
            Box::pin(lease_is_removed_when_the_herdr_subscription_ends())
        }),
        (
            "lease_is_removed_when_the_service_stops",
            || Box::pin(lease_is_removed_when_the_service_stops()),
        ),
        ("install_does_not_install_the_plugin_when_the_host_service_sends_directly", || {
            Box::pin(install_does_not_install_the_plugin_when_the_host_service_sends_directly())
        }),
        ("status_reports_direct_push_for_each_plugin_state", || {
            Box::pin(status_reports_direct_push_for_each_plugin_state())
        }),
        ("install_with_a_legacy_plugin_keeps_the_old_behaviour", || {
            Box::pin(install_with_a_legacy_plugin_keeps_the_old_behaviour())
        }),
        ("update_muted_reply_has_no_direct_push", || Box::pin(update_muted_reply_has_no_direct_push())),
    ]
}

fn main() -> std::process::ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some(FAKE_HERDR) {
        return std::process::ExitCode::from(fake_herdr(&arguments[1..]) as u8);
    }
    // cargo test 傳進來的篩選字(不是 `--` 開頭的參數)。
    let filters: Vec<&String> = arguments.iter().filter(|argument| !argument.starts_with('-')).collect();
    let selected: Vec<TestCase> = tests()
        .into_iter()
        .filter(|(name, _)| filters.is_empty() || filters.iter().any(|filter| name.contains(filter.as_str())))
        .collect();
    println!("\nrunning {} tests", selected.len());
    let mut failures = Vec::new();
    for (name, test) in &selected {
        let outcome = std::panic::catch_unwind(|| {
            tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(test());
        });
        println!("test {name} ... {}", if outcome.is_ok() { "ok" } else { "FAILED" });
        if outcome.is_err() {
            failures.push(*name);
        }
    }
    println!("\ntest result: {} passed; {} failed", selected.len() - failures.len(), failures.len());
    if failures.is_empty() {
        std::process::ExitCode::SUCCESS
    } else {
        println!("failures: {failures:?}");
        std::process::ExitCode::FAILURE
    }
}
