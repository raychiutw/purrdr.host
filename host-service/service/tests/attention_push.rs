//! 卡住與仍在等你的推播(raychiutw/purrdr.host#31):觀察者定時讀 working Agent 的畫面指紋、看 blocked 多久，
//! 經 `~/.purrdr/push-<裝置>/pairing` 送後台 `status: stuck／reminder`(形狀同主機外掛，與 raychiutw/purrdr.server#15 契約一致)。
//! 門檻注入短的時間;規則本身(10／15 分鐘、只送一次)由 tests/attention.rs 以明確時間點測。

mod install_support;
mod support;

use std::path::Path;
use std::time::Duration;

use install_support::push_backend::FakePushBackend;
use purrdr_host_service::agent_observer::ObserverOptions;
use serde_json::{Value, json};
use support::{FakeHerdr, recorded, start_service_with_observer};
use tempfile::TempDir;

const PHONE_KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const TABLET_KEY: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const WATCH_KEY: &str = "3333333333333333333333333333333333333333333333333333333333333333";
/// session.snapshot.ndjson 裡 w1:pA 的 session。
const PA_SESSION: &str = "00000000-0000-0000-0000-000000000002";

fn options() -> ObserverOptions {
    ObserverOptions {
        reconnect_initial: Duration::from_millis(20),
        reconnect_maximum: Duration::from_millis(80),
        screen_check_interval: Duration::from_millis(20),
        stuck_after: Duration::from_millis(150),
        reminder_after: Duration::from_millis(150),
    }
}

fn pair(home: &Path, device: &str, endpoint: &str, key: &str, muted: &[&str]) {
    let directory = home.join(".purrdr").join(format!("push-{device}"));
    std::fs::create_dir_all(&directory).unwrap();
    let muted: String = muted.iter().map(|session| format!("muted={session}\n")).collect();
    std::fs::write(directory.join("pairing"), format!("purrdr-push 1\nendpoint={endpoint}\nkey={key}\n{muted}")).unwrap();
}

/// session.snapshot:w1:pA、w1:pB working,w1:p6 idle;畫面一直不變。
async fn herdr() -> FakeHerdr {
    FakeHerdr::start(vec![
        ("ping", recorded("ping.ndjson")),
        ("session.snapshot", recorded("session.snapshot.ndjson")),
        ("events.subscribe", recorded("events.subscribe.started.ndjson")),
        ("pane.read", recorded("pane.read.question.ndjson")),
    ])
    .await
}

async fn wait_for(backend: &FakePushBackend, condition: impl Fn(&[Value]) -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !condition(&backend.bodies()) {
        assert!(tokio::time::Instant::now() < deadline, "等不到推播:{:?}", backend.bodies());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn sent(bodies: &[Value], key: &str, status: &str, pane: &str) -> usize {
    bodies
        .iter()
        .filter(|body| body["key"] == key && body["status"] == status && body["pane"] == pane)
        .count()
}

#[tokio::test]
async fn an_unchanged_working_agent_is_reported_stuck_once_to_each_device() {
    let home = TempDir::new().unwrap();
    let backend = FakePushBackend::start();
    pair(home.path(), "phone", &backend.endpoint(), PHONE_KEY, &[]);
    pair(home.path(), "watch", &backend.endpoint(), WATCH_KEY, &[PA_SESSION]);
    let herdr = herdr().await;
    let _service = start_service_with_observer(herdr.socket.clone(), None, Some(home.path().to_owned()), options()).await;

    wait_for(&backend, |bodies| {
        sent(bodies, PHONE_KEY, "stuck", "w1:pA") == 1 && sent(bodies, PHONE_KEY, "stuck", "w1:pB") == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let bodies = backend.bodies();
    assert_eq!(sent(&bodies, PHONE_KEY, "stuck", "w1:pA"), 1, "{bodies:?}");
    assert_eq!(sent(&bodies, PHONE_KEY, "stuck", "w1:pB"), 1, "{bodies:?}");
    // 只帶密鑰、workspace 名稱、狀態與 pane id(後台多一個欄位就回 400)。
    assert!(bodies.contains(&json!({"key": PHONE_KEY, "workspace": "garden-journal", "status": "stuck", "pane": "w1:pA", "tab": "claude"})));
    // watch 靜音了 w1:pA 的聊天。
    assert_eq!(sent(&bodies, WATCH_KEY, "stuck", "w1:pA"), 0);
    assert_eq!(sent(&bodies, WATCH_KEY, "stuck", "w1:pB"), 1);
    assert!(herdr.methods().iter().any(|method| method == "pane.read"));
}

#[tokio::test]
async fn an_agent_still_needing_you_is_reminded_once() {
    let home = TempDir::new().unwrap();
    let backend = FakePushBackend::start();
    pair(home.path(), "phone", &backend.endpoint(), PHONE_KEY, &[]);
    let herdr = herdr().await;
    let _service = start_service_with_observer(herdr.socket.clone(), None, Some(home.path().to_owned()), options()).await;
    herdr.wait_until(|herdr| herdr.open_subscriptions() == 1).await;

    herdr.push_event(
        json!({"event": "pane.agent_status_changed", "data": {"agent": "claude", "agent_status": "blocked", "pane_id": "w1:pB", "workspace_id": "w1"}}),
    );
    wait_for(&backend, |bodies| sent(bodies, PHONE_KEY, "reminder", "w1:pB") == 1).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(sent(&backend.bodies(), PHONE_KEY, "reminder", "w1:pB"), 1);
    assert_eq!(
        backend.bodies().into_iter().find(|body| body["status"] == "reminder"),
        Some(json!({"key": PHONE_KEY, "workspace": "garden-journal", "status": "reminder", "pane": "w1:pB", "tab": "claude"}))
    );
}

#[tokio::test]
async fn a_revoked_pairing_is_removed_and_others_still_get_the_push() {
    let home = TempDir::new().unwrap();
    let backend = FakePushBackend::start();
    backend.respond_to_key(TABLET_KEY, 410);
    pair(home.path(), "phone", &backend.endpoint(), PHONE_KEY, &[]);
    pair(home.path(), "tablet", &backend.endpoint(), TABLET_KEY, &[]);
    let herdr = herdr().await;
    let _service = start_service_with_observer(herdr.socket.clone(), None, Some(home.path().to_owned()), options()).await;

    wait_for(&backend, |bodies| {
        sent(bodies, PHONE_KEY, "stuck", "w1:pA") == 1 && sent(bodies, TABLET_KEY, "stuck", "w1:pA") >= 1
    })
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while home.path().join(".purrdr").join("push-tablet").exists() {
        assert!(tokio::time::Instant::now() < deadline, "被撤銷的配對沒有刪掉");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(home.path().join(".purrdr").join("push-phone").join("pairing").exists());
}
