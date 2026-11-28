//! 主機心跳與睡眠回報(raychiutw/purrdr.host#99、ADR-0017):不連網路、不讓機器睡眠。
//! 狀態機以明確的 `Instant` 驅動;排程以很短的真實時間跑，事件用 channel 等，不靠固定 sleep。

use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use purrdr_host_service::host_heartbeat::{HeartbeatTransport, HostHeartbeat, HostState, Schedule, SleepAck, SleepSignal, run};
use serde_json::{Value, json};

const KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const OTHER_KEY: &str = "2222222222222222222222222222222222222222222222222222222222222222";

type Reply = Result<(u16, String), String>;

/// 記下每次請求;回應依序取自腳本(用完回 `{"needed":true}`)。
#[derive(Clone)]
struct FakeTransport {
    sent: Arc<Mutex<Vec<(String, Value)>>>,
    replies: Arc<Mutex<Vec<Reply>>>,
    /// 依序套在每次請求上的延遲(用完不延遲),模擬慢的後台。
    delays: Arc<Mutex<Vec<Duration>>>,
    notify: Option<mpsc::Sender<()>>,
}

impl FakeTransport {
    fn new() -> Self {
        FakeTransport {
            sent: Arc::default(),
            replies: Arc::default(),
            delays: Arc::default(),
            notify: None,
        }
    }
    fn reply_with(self, replies: Vec<Reply>) -> Self {
        *self.replies.lock().unwrap() = replies;
        self
    }
    fn delay_with(self, delays: Vec<Duration>) -> Self {
        *self.delays.lock().unwrap() = delays;
        self
    }
    fn sent(&self) -> Vec<(String, Value)> {
        self.sent.lock().unwrap().clone()
    }
}

impl HeartbeatTransport for FakeTransport {
    fn post(&mut self, endpoint: &str, body: &str) -> Reply {
        self.sent.lock().unwrap().push((endpoint.to_owned(), serde_json::from_str(body).unwrap()));
        let delay = {
            let mut delays = self.delays.lock().unwrap();
            if delays.is_empty() { Duration::ZERO } else { delays.remove(0) }
        };
        std::thread::sleep(delay);
        let reply = {
            let mut replies = self.replies.lock().unwrap();
            if replies.is_empty() {
                Ok((200, r#"{"needed":true}"#.to_owned()))
            } else {
                replies.remove(0)
            }
        };
        if let Some(notify) = &self.notify {
            let _ = notify.send(());
        }
        reply
    }
}

fn needed(value: bool) -> Reply {
    Ok((200, json!({"needed": value}).to_string()))
}

fn pair(home: &Path, device: &str, endpoint: &str, key: &str) {
    let directory = home.join(".purrdr").join(format!("push-{device}"));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("pairing"), format!("endpoint={endpoint}\nkey={key}\n")).unwrap();
}

fn machine(home: &Path, transport: &FakeTransport) -> HostHeartbeat<FakeTransport> {
    HostHeartbeat::new(home, transport.clone(), Duration::from_secs(3600))
}

#[test]
fn beat_posts_only_key_and_state_to_the_origin_of_the_pairing_endpoint() {
    let home = tempfile::tempdir().unwrap();
    pair(home.path(), "phone", "https://api.example.test:8443/v1/push?x=1", KEY);
    let transport = FakeTransport::new();

    machine(home.path(), &transport).beat(HostState::Online, Instant::now());

    assert_eq!(
        transport.sent(),
        vec![("https://api.example.test:8443/v1/heartbeat".to_owned(), json!({"key": KEY, "state": "online"}))]
    );
    let body = &transport.sent()[0].1;
    assert_eq!(body.as_object().unwrap().len(), 2, "只能有 key 與 state");
}

#[test]
fn sleeping_state_is_reported_as_sleeping() {
    let home = tempfile::tempdir().unwrap();
    pair(home.path(), "phone", "https://api.example.test/v1/push", KEY);
    let transport = FakeTransport::new();

    machine(home.path(), &transport).beat(HostState::Sleeping, Instant::now());

    assert_eq!(transport.sent()[0].1, json!({"key": KEY, "state": "sleeping"}));
}

#[test]
fn every_pairing_is_beaten_and_no_pairing_means_no_request() {
    let home = tempfile::tempdir().unwrap();
    let transport = FakeTransport::new();
    let mut heartbeat = machine(home.path(), &transport);
    heartbeat.beat(HostState::Online, Instant::now());
    assert!(transport.sent().is_empty());

    pair(home.path(), "phone", "https://a.example.test/v1/push", KEY);
    pair(home.path(), "tablet", "https://b.example.test/v1/push", OTHER_KEY);
    heartbeat.beat(HostState::Online, Instant::now());

    assert_eq!(transport.sent().len(), 2);
}

#[test]
fn not_needed_stops_that_pairing_until_the_recheck_interval() {
    let home = tempfile::tempdir().unwrap();
    pair(home.path(), "phone", "https://a.example.test/v1/push", KEY);
    pair(home.path(), "tablet", "https://b.example.test/v1/push", OTHER_KEY);
    let transport = FakeTransport::new().reply_with(vec![needed(false), needed(true)]);
    let mut heartbeat = machine(home.path(), &transport);
    let start = Instant::now();

    heartbeat.beat(HostState::Online, start);
    assert_eq!(transport.sent().len(), 2);

    // 5 分鐘後:只剩還需要的那組。
    heartbeat.beat(HostState::Online, start + Duration::from_secs(300));
    let sent = transport.sent();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[2].1["key"], OTHER_KEY);

    // 睡眠回報也不送給已停止的。
    heartbeat.beat(HostState::Sleeping, start + Duration::from_secs(600));
    assert_eq!(transport.sent().len(), 4);
    assert_eq!(transport.sent()[3].1["key"], OTHER_KEY);

    // 滿 1 小時再問一次。
    heartbeat.beat(HostState::Online, start + Duration::from_secs(3600));
    let sent = transport.sent();
    assert_eq!(sent.len(), 6);
    assert_eq!(sent[4].1["key"], KEY);
}

#[test]
fn a_fresh_instance_asks_again_like_a_service_restart() {
    let home = tempfile::tempdir().unwrap();
    pair(home.path(), "phone", "https://a.example.test/v1/push", KEY);
    let transport = FakeTransport::new().reply_with(vec![needed(false)]);
    machine(home.path(), &transport).beat(HostState::Online, Instant::now());

    machine(home.path(), &transport).beat(HostState::Online, Instant::now());

    assert_eq!(transport.sent().len(), 2);
}

#[test]
fn network_failure_or_bad_reply_does_not_stop_and_is_not_retried_within_the_beat() {
    let home = tempfile::tempdir().unwrap();
    pair(home.path(), "phone", "https://a.example.test/v1/push", KEY);
    let transport = FakeTransport::new().reply_with(vec![Err("連不上".to_owned()), Ok((500, String::new())), Ok((200, "not json".to_owned()))]);
    let mut heartbeat = machine(home.path(), &transport);
    let start = Instant::now();

    heartbeat.beat(HostState::Online, start);
    assert_eq!(transport.sent().len(), 1, "同一次不重試");
    heartbeat.beat(HostState::Online, start + Duration::from_secs(300));
    heartbeat.beat(HostState::Online, start + Duration::from_secs(600));
    heartbeat.beat(HostState::Online, start + Duration::from_secs(900));

    assert_eq!(transport.sent().len(), 4, "失敗與看不懂的回應都不當作『不需要』，下個週期照送");
}

#[test]
fn revoked_pairing_is_removed() {
    let home = tempfile::tempdir().unwrap();
    pair(home.path(), "phone", "https://a.example.test/v1/push", KEY);
    let transport = FakeTransport::new().reply_with(vec![Ok((410, String::new()))]);

    machine(home.path(), &transport).beat(HostState::Online, Instant::now());

    assert!(!home.path().join(".purrdr/push-phone").exists());
}

// --- 排程(短的真實時間) ---

const WAIT: Duration = Duration::from_secs(5);

fn schedule() -> Schedule {
    Schedule {
        interval: Duration::from_millis(400),
        recheck: Duration::from_secs(3600),
    }
}

fn spawn_run(
    home: &Path,
    transport: &FakeTransport,
    signals: Option<tokio::sync::mpsc::UnboundedReceiver<SleepSignal>>,
) -> (tokio::task::JoinHandle<()>, mpsc::Receiver<()>) {
    let (notify, beats) = mpsc::channel();
    let mut transport = transport.clone();
    transport.notify = Some(notify);
    (tokio::spawn(run(home.to_owned(), transport, schedule(), signals)), beats)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schedule_beats_immediately_then_every_interval_and_stops_when_not_needed() {
    let home = tempfile::tempdir().unwrap();
    pair(home.path(), "phone", "https://a.example.test/v1/push", KEY);
    let transport = FakeTransport::new().reply_with(vec![needed(true), needed(false)]);
    let (task, beats) = spawn_run(home.path(), &transport, None);

    beats.recv_timeout(WAIT).unwrap();
    assert_eq!(transport.sent().len(), 1, "啟動就先送一次");
    beats.recv_timeout(WAIT).unwrap();
    assert_eq!(transport.sent().len(), 2);

    // 第二次回 needed=false:之後(遠超過數個週期)不再送。
    assert!(beats.recv_timeout(Duration::from_millis(1500)).is_err());
    assert_eq!(transport.sent().len(), 2);
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schedule_does_not_retry_densely_after_a_network_failure() {
    let home = tempfile::tempdir().unwrap();
    pair(home.path(), "phone", "https://a.example.test/v1/push", KEY);
    let transport = FakeTransport::new().reply_with(vec![Err("連不上".to_owned())]);
    let (task, beats) = spawn_run(home.path(), &transport, None);

    beats.recv_timeout(WAIT).unwrap();
    // 週期是 400ms:失敗後 200ms 內不能有第二次。
    assert!(beats.recv_timeout(Duration::from_millis(200)).is_err());
    beats.recv_timeout(WAIT).unwrap();
    assert_eq!(transport.sent().len(), 2);
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schedule_reports_sleeping_before_sleep_acknowledges_after_and_reports_online_on_wake() {
    let home = tempfile::tempdir().unwrap();
    pair(home.path(), "phone", "https://a.example.test/v1/push", KEY);
    let transport = FakeTransport::new();
    let (signals, receiver) = tokio::sync::mpsc::unbounded_channel();
    let (task, beats) = spawn_run(home.path(), &transport, Some(receiver));
    beats.recv_timeout(WAIT).unwrap();

    let (ack, acknowledged) = SleepAck::channel();
    signals.send(SleepSignal::WillSleep(ack)).unwrap();
    // 睡眠回報送完才放行睡眠。
    acknowledged.recv_timeout(WAIT).unwrap();
    assert_eq!(transport.sent().last().unwrap().1, json!({"key": KEY, "state": "sleeping"}));

    signals.send(SleepSignal::DidWake).unwrap();
    // 醒來立刻送 online(不等下個週期):週期是 400ms,所以 100ms 內要送出,且是 online。
    let before = transport.sent().len();
    let deadline = Instant::now() + WAIT;
    while transport.sent().len() == before && Instant::now() < deadline {
        let _ = beats.recv_timeout(Duration::from_millis(50));
    }
    assert_eq!(transport.sent().last().unwrap().1, json!({"key": KEY, "state": "online"}));
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sleep_report_is_not_held_up_by_a_heartbeat_in_flight() {
    // 第一次心跳卡 3 秒(慢的後台);macOS 只等 5 秒就放行睡眠,睡眠回報不能排在它後面。
    let home = tempfile::tempdir().unwrap();
    pair(home.path(), "phone", "https://a.example.test/v1/push", KEY);
    let transport = FakeTransport::new().delay_with(vec![Duration::from_secs(3)]);
    let (signals, receiver) = tokio::sync::mpsc::unbounded_channel();
    let (task, _beats) = spawn_run(home.path(), &transport, Some(receiver));
    while transport.sent().is_empty() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let (ack, acknowledged) = SleepAck::channel();
    signals.send(SleepSignal::WillSleep(ack)).unwrap();
    acknowledged.recv_timeout(Duration::from_secs(1)).expect("睡眠回報被進行中的心跳擋住");
    assert!(transport.sent().iter().any(|(_, body)| body["state"] == "sleeping"), "{:?}", transport.sent());
    task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schedule_without_a_sleep_source_keeps_beating() {
    let home = tempfile::tempdir().unwrap();
    pair(home.path(), "phone", "https://a.example.test/v1/push", KEY);
    let transport = FakeTransport::new();
    let (task, beats) = spawn_run(home.path(), &transport, None);

    beats.recv_timeout(WAIT).unwrap();
    beats.recv_timeout(WAIT).unwrap();
    task.abort();
}
