//! Agent 觀察者與 Send When Done 佇列(raychiutw/purrdr.host#30):主機服務常駐訂閱 herdr 事件，
//! Agent 從 working 變成 idle／done 時送出佇列第一則(經 expected_session 比對),app 沒連線也照樣運作。

mod install_support;
mod support;

use std::path::PathBuf;
use std::time::Duration;

use install_support::push_backend::FakePushBackend;
use purrdr_host_service::agent_observer::ObserverOptions;
use serde_json::{Value, json};
use support::{FakeHerdr, TestClient, TestHostService, recorded, start_service_with_observer};

fn hello(request_id: &str) -> Value {
    json!({"type": "hello", "request_id": request_id, "protocol_version": 1})
}

/// session.snapshot.ndjson:w1:pA、w1:pB 是 working,w1:p6 是 idle。
const PA_SESSION: &str = "00000000-0000-0000-0000-000000000002";
const PB_SESSION: &str = "00000000-0000-0000-0000-000000000003";

fn options() -> ObserverOptions {
    ObserverOptions {
        reconnect_initial: Duration::from_millis(20),
        reconnect_maximum: Duration::from_millis(80),
        ..ObserverOptions::default()
    }
}

async fn herdr() -> FakeHerdr {
    FakeHerdr::start(vec![
        ("ping", recorded("ping.ndjson")),
        ("session.snapshot", recorded("session.snapshot.ndjson")),
        ("events.subscribe", recorded("events.subscribe.started.ndjson")),
        ("agent.prompt", recorded("agent.prompt.ndjson")),
    ])
    .await
}

/// 觀察者訂閱好、讀過 snapshot。
async fn observing(herdr: &FakeHerdr, state_directory: Option<PathBuf>) -> TestHostService {
    let service = start_service_with_observer(herdr.socket.clone(), state_directory, None, options()).await;
    herdr
        .wait_until(|herdr| herdr.open_subscriptions() == 1 && herdr.methods().iter().any(|method| method == "session.snapshot"))
        .await;
    service
}

const PHONE_KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const WATCH_KEY: &str = "3333333333333333333333333333333333333333333333333333333333333333";

fn pair(home: &std::path::Path, device: &str, endpoint: &str, key: &str, muted: &[&str]) {
    let directory = home.join(".purrdr").join(format!("push-{device}"));
    std::fs::create_dir_all(&directory).unwrap();
    let muted: String = muted.iter().map(|session| format!("muted={session}\n")).collect();
    std::fs::write(directory.join("pairing"), format!("purrdr-push 1\nendpoint={endpoint}\nkey={key}\n{muted}")).unwrap();
}

async fn observing_with_home(herdr: &FakeHerdr, home: &std::path::Path) -> TestHostService {
    let service = start_service_with_observer(herdr.socket.clone(), None, Some(home.to_owned()), options()).await;
    herdr
        .wait_until(|herdr| herdr.open_subscriptions() == 1 && herdr.methods().iter().any(|method| method == "session.snapshot"))
        .await;
    service
}

async fn wait_for_pushes(backend: &FakePushBackend, count: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while backend.bodies().len() < count {
        assert!(tokio::time::Instant::now() < deadline, "等不到推播:{:?}", backend.bodies());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn connect(service: &TestHostService) -> TestClient {
    let mut client = TestClient::connect(service).await;
    request(&mut client, hello("r1")).await;
    client
}

fn status(pane_id: &str, agent_status: &str) -> Value {
    json!({"event": "pane.agent_status_changed", "data": {"agent": "claude", "agent_status": agent_status, "pane_id": pane_id, "workspace_id": "w1"}})
}

fn prompts(herdr: &FakeHerdr) -> Vec<Value> {
    herdr
        .requests()
        .into_iter()
        .filter(|request| request["method"] == "agent.prompt")
        .map(|request| request["params"].clone())
        .collect()
}

async fn add(client: &mut TestClient, pane_id: &str, text: &str, expected_session: &str) -> String {
    let reply = request(
        client,
        json!({"type": "queue.add", "request_id": "add", "pane_id": pane_id, "text": text, "expected_session": expected_session}),
    )
    .await;
    assert_eq!(reply["type"], "queue.add.result", "{reply}");
    reply["item_id"].as_str().unwrap().to_owned()
}

/// 送出請求並等它的回應;queue.changed 由另一個 task 轉送，先後不固定，略過。
async fn request(client: &mut TestClient, message: Value) -> Value {
    let request_id = message["request_id"].clone();
    client.send(message).await;
    loop {
        let reply = client.receive_within(Duration::from_secs(5)).await;
        if reply["request_id"] == request_id {
            return reply;
        }
    }
}

/// 下一個符合條件的 queue.changed 推送(略過其他訊息)。
async fn change_where(client: &mut TestClient, condition: impl Fn(&Value) -> bool) -> Value {
    loop {
        let message = client.receive_within(Duration::from_secs(5)).await;
        if message["type"] == "queue.changed" && condition(&message) {
            return message;
        }
    }
}

fn is_sent(change: &Value) -> bool {
    change.get("sent_item_id").is_some()
}

#[tokio::test]
async fn the_first_queued_prompt_is_sent_when_the_agent_finishes() {
    let herdr = herdr().await;
    let service = observing(&herdr, None).await;
    let mut client = connect(&service).await;
    let first = add(&mut client, "w1:pA", "run the tests", PA_SESSION).await;
    let second = add(&mut client, "w1:pA", "then commit", PA_SESSION).await;

    herdr.push_event(status("w1:pA", "idle"));
    let sent = change_where(&mut client, is_sent).await;

    assert_eq!(prompts(&herdr), [json!({"target": "w1:pA", "text": "run the tests"})]);
    assert_eq!(sent["sent_item_id"], first.as_str());
    assert_eq!(
        sent["queue"],
        json!({"pane_id": "w1:pA", "items": [{"item_id": second, "text": "then commit", "expected_session": PA_SESSION}]})
    );

    herdr.push_event(status("w1:pA", "working"));
    herdr.push_event(status("w1:pA", "done"));
    change_where(&mut client, is_sent).await;
    assert_eq!(prompts(&herdr).len(), 2);
    assert_eq!(prompts(&herdr)[1]["text"], "then commit");
}

#[tokio::test]
async fn nothing_is_sent_while_the_agent_needs_you() {
    let herdr = herdr().await;
    let service = observing(&herdr, None).await;
    let mut client = connect(&service).await;
    add(&mut client, "w1:pA", "after the question", PA_SESSION).await;
    add(&mut client, "w1:pB", "marker", PB_SESSION).await;

    herdr.push_event(status("w1:pA", "blocked"));
    herdr.push_event(status("w1:pA", "idle"));
    herdr.push_event(status("w1:pB", "idle"));
    herdr.wait_until(|herdr| !prompts(herdr).is_empty()).await;

    // pB 送出時，前面的 pA 事件都處理完了:blocked 之後的 idle 不是「做完」。
    assert_eq!(prompts(&herdr), [json!({"target": "w1:pB", "text": "marker"})]);

    herdr.push_event(status("w1:pA", "working"));
    herdr.push_event(status("w1:pA", "idle"));
    herdr.wait_until(|herdr| prompts(herdr).len() == 2).await;
    assert_eq!(prompts(&herdr)[1], json!({"target": "w1:pA", "text": "after the question"}));
}

#[tokio::test]
async fn a_different_session_pauses_the_queue_without_sending() {
    let herdr = herdr().await;
    let home = tempfile::TempDir::new().unwrap();
    let backend = FakePushBackend::start();
    pair(home.path(), "phone", &backend.endpoint(), PHONE_KEY, &[]);
    let service = observing_with_home(&herdr, home.path()).await;
    let mut client = connect(&service).await;
    let item = add(&mut client, "w1:pA", "for the old chat", "00000000-0000-0000-0000-0000000000ff").await;

    herdr.push_event(status("w1:pA", "idle"));
    let paused = change_where(&mut client, |change| change["queue"].get("paused").is_some()).await;

    assert!(prompts(&herdr).is_empty());
    assert_eq!(paused["queue"]["paused"], "session_changed");
    assert_eq!(paused.get("sent_item_id"), None);
    let listed = request(&mut client, json!({"type": "queue.list", "request_id": "r3", "pane_id": "w1:pA"})).await;
    assert_eq!(
        listed["queues"],
        json!([{"pane_id": "w1:pA", "paused": "session_changed", "items": [{"item_id": item, "text": "for the old chat", "expected_session": "00000000-0000-0000-0000-0000000000ff"}]}])
    );
    wait_for_pushes(&backend, 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let bodies = backend.bodies();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert_eq!(bodies[0]["key"], PHONE_KEY);
    assert_eq!(bodies[0]["status"], "queue_paused");
    assert_eq!(bodies[0]["pane"], "w1:pA");
    assert_eq!(bodies[0]["workspace"], bodies[0]["workspace"].as_str().unwrap());
    assert!(bodies[0].get("tab").is_some(), "{bodies:?}");
}

#[tokio::test]
async fn pausing_pushes_once_per_pause_skips_muted_devices_and_pushes_again_after_resuming() {
    let herdr = herdr().await;
    let home = tempfile::TempDir::new().unwrap();
    let backend = FakePushBackend::start();
    pair(home.path(), "phone", &backend.endpoint(), PHONE_KEY, &[]);
    pair(home.path(), "watch", &backend.endpoint(), WATCH_KEY, &[PA_SESSION]);
    let service = observing_with_home(&herdr, home.path()).await;
    let mut client = connect(&service).await;
    let stale = add(&mut client, "w1:pA", "for the old chat", "00000000-0000-0000-0000-0000000000ff").await;
    herdr.push_event(status("w1:pA", "idle"));
    change_where(&mut client, |change| change["queue"].get("paused").is_some()).await;
    wait_for_pushes(&backend, 1).await;

    // 暫停期間再變成 working/idle:仍是同一次暫停，不再送。
    herdr.push_event(status("w1:pA", "working"));
    herdr.push_event(status("w1:pA", "idle"));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(backend.bodies().len(), 1, "{:?}", backend.bodies());
    assert_eq!(backend.bodies()[0]["key"], PHONE_KEY, "靜音該聊天的裝置不送");

    // 移除舊項目恢復後再暫停:可以再送。
    request(
        &mut client,
        json!({"type": "queue.remove", "request_id": "r4", "pane_id": "w1:pA", "item_id": stale}),
    )
    .await;
    add(&mut client, "w1:pA", "for another old chat", "00000000-0000-0000-0000-0000000000fe").await;
    herdr.push_event(status("w1:pA", "working"));
    herdr.push_event(status("w1:pA", "idle"));
    wait_for_pushes(&backend, 2).await;
}
#[tokio::test]
async fn queues_have_limits_and_only_known_items_can_be_changed() {
    let herdr = herdr().await;
    let service = observing(&herdr, None).await;
    let mut client = connect(&service).await;
    let mut items = Vec::new();
    for number in 0..20 {
        items.push(add(&mut client, "w1:p6", &format!("step {number}"), PA_SESSION).await);
    }

    let full = request(
        &mut client,
        json!({"type": "queue.add", "request_id": "r2", "pane_id": "w1:p6", "text": "one more", "expected_session": PA_SESSION}),
    )
    .await;
    let too_long = request(
        &mut client,
        json!({"type": "queue.add", "request_id": "r3", "pane_id": "w1:pB", "text": "x".repeat(64 * 1024 + 1), "expected_session": PB_SESSION}),
    )
    .await;
    let unknown = request(
        &mut client,
        json!({"type": "queue.remove", "request_id": "r4", "pane_id": "w1:p6", "item_id": "nope"}),
    )
    .await;
    let partial = request(
        &mut client,
        json!({"type": "queue.reorder", "request_id": "r5", "pane_id": "w1:p6", "item_ids": items[..19].to_vec()}),
    )
    .await;
    let mut reversed = items.clone();
    reversed.reverse();
    let reordered = request(
        &mut client,
        json!({"type": "queue.reorder", "request_id": "r6", "pane_id": "w1:p6", "item_ids": reversed}),
    )
    .await;
    let removed = request(
        &mut client,
        json!({"type": "queue.remove", "request_id": "r7", "pane_id": "w1:p6", "item_id": items[0]}),
    )
    .await;
    let listed = request(&mut client, json!({"type": "queue.list", "request_id": "r8"})).await;

    assert_eq!(full["code"], "queue_full");
    assert_eq!(too_long["code"], "invalid_message");
    assert_eq!(unknown["code"], "queue_item_unknown");
    assert_eq!(partial["code"], "queue_item_unknown");
    assert_eq!(reordered, json!({"type": "queue.reorder.result", "request_id": "r6"}));
    assert_eq!(removed, json!({"type": "queue.remove.result", "request_id": "r7"}));
    let texts: Vec<&str> = listed["queues"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts.len(), 19);
    assert_eq!((texts[0], texts[18]), ("step 19", "step 1"));
}

#[tokio::test]
async fn queues_survive_a_restart() {
    let state = tempfile::tempdir().unwrap();
    let herdr = herdr().await;
    {
        let service = observing(&herdr, Some(state.path().to_owned())).await;
        let mut client = connect(&service).await;
        add(&mut client, "w1:pA", "still here", PA_SESSION).await;
    }
    let restarted_herdr = herdr_with_new_socket().await;
    let service = observing(&restarted_herdr, Some(state.path().to_owned())).await;
    let mut client = connect(&service).await;

    let listed = request(&mut client, json!({"type": "queue.list", "request_id": "r2"})).await;
    assert_eq!(listed["queues"][0]["items"][0]["text"], "still here");

    restarted_herdr.push_event(status("w1:pA", "idle"));
    restarted_herdr.wait_until(|herdr| prompts(herdr).len() == 1).await;
    assert_eq!(prompts(&restarted_herdr)[0]["text"], "still here");
}

async fn herdr_with_new_socket() -> FakeHerdr {
    herdr().await
}

#[tokio::test]
async fn the_observer_resubscribes_after_herdr_ends_the_subscription() {
    let herdr = herdr().await;
    let service = observing(&herdr, None).await;
    let mut client = connect(&service).await;
    add(&mut client, "w1:pA", "after reconnecting", PA_SESSION).await;

    herdr.end_subscriptions(None);
    herdr
        .wait_until(|herdr| herdr.open_subscriptions() == 1 && herdr.methods().iter().filter(|method| *method == "events.subscribe").count() == 2)
        .await;
    herdr.push_event(status("w1:pA", "idle"));

    herdr.wait_until(|herdr| prompts(herdr).len() == 1).await;
}

#[tokio::test]
async fn read_only_devices_see_queues_but_cannot_change_them_and_audit_skips_the_text() {
    let herdr = herdr().await;
    let service = observing(&herdr, None).await;
    let mut writer = connect(&service).await;
    add(&mut writer, "w1:pA", "SECRET-PROMPT-TEXT", PA_SESSION).await;
    let read_only = service.pair_device("iPad", true);
    let mut reader = TestClient::connect_with_token(&service, Some(&read_only.device_token)).await;
    reader.request(hello("r1")).await;

    let denied = reader
        .request(json!({"type": "queue.add", "request_id": "r2", "pane_id": "w1:pA", "text": "nope", "expected_session": PA_SESSION}))
        .await;
    let listed = request(&mut reader, json!({"type": "queue.list", "request_id": "r3"})).await;

    assert_eq!(denied["code"], "read_only_device");
    assert_eq!(listed["queues"][0]["items"][0]["text"], "SECRET-PROMPT-TEXT");
    let audit = service.audit_log();
    assert!(audit.contains("\"operation\":\"queue.add\""), "{audit}");
    assert!(!audit.contains("SECRET-PROMPT-TEXT"));
}

#[tokio::test]
async fn every_queued_prompt_names_the_session_it_is_for() {
    let herdr = herdr().await;
    let service = observing(&herdr, None).await;
    let mut client = connect(&service).await;

    let reply = request(
        &mut client,
        json!({"type": "queue.add", "request_id": "r2", "pane_id": "w1:pA", "text": "for whichever chat"}),
    )
    .await;

    assert_eq!(reply["code"], "invalid_message", "{reply}");
}

#[tokio::test]
async fn adding_behind_a_paused_prompt_keeps_the_queue_paused() {
    let herdr = herdr().await;
    let service = observing(&herdr, None).await;
    let mut client = connect(&service).await;
    let stale = add(&mut client, "w1:pA", "for the old chat", "00000000-0000-0000-0000-0000000000ff").await;
    herdr.push_event(status("w1:pA", "idle"));
    change_where(&mut client, |change| change["queue"].get("paused").is_some()).await;

    add(&mut client, "w1:pA", "for this chat", PA_SESSION).await;
    let listed = request(&mut client, json!({"type": "queue.list", "request_id": "r3", "pane_id": "w1:pA"})).await;
    assert_eq!(listed["queues"][0]["paused"], "session_changed", "{listed}");

    request(
        &mut client,
        json!({"type": "queue.remove", "request_id": "r4", "pane_id": "w1:pA", "item_id": stale}),
    )
    .await;
    let listed = request(&mut client, json!({"type": "queue.list", "request_id": "r5", "pane_id": "w1:pA"})).await;
    assert_eq!(listed["queues"][0].get("paused"), None, "{listed}");
}

#[tokio::test]
async fn an_agent_that_finished_while_herdr_was_disconnected_still_gets_its_prompt() {
    let herdr = herdr().await;
    let service = observing(&herdr, None).await;
    let mut client = connect(&service).await;
    add(&mut client, "w1:pA", "after reconnecting", PA_SESSION).await;

    let mut finished = recorded("session.snapshot.ndjson");
    for agent in finished["result"]["snapshot"]["agents"].as_array_mut().unwrap() {
        if agent["pane_id"] == "w1:pA" {
            agent["agent_status"] = json!("idle");
        }
    }
    herdr.set_reply("session.snapshot", finished);
    herdr.end_subscriptions(None);

    herdr.wait_until(|herdr| prompts(herdr).len() == 1).await;
    assert_eq!(prompts(&herdr)[0], json!({"target": "w1:pA", "text": "after reconnecting"}));
}

#[tokio::test]
async fn a_slow_queue_paused_push_does_not_block_the_next_pane() {
    // session.snapshot 每次要 1 秒:暫停推播要先抓 snapshot,若在事件迴圈裡等,下一個 pane 的送出會被拖慢。
    let herdr = FakeHerdr::start_delayed(vec![
        ("ping", recorded("ping.ndjson"), Duration::ZERO),
        ("session.snapshot", recorded("session.snapshot.ndjson"), Duration::from_secs(1)),
        ("events.subscribe", recorded("events.subscribe.started.ndjson"), Duration::ZERO),
        ("agent.prompt", recorded("agent.prompt.ndjson"), Duration::ZERO),
    ])
    .await;
    let home = tempfile::TempDir::new().unwrap();
    let backend = FakePushBackend::start();
    pair(home.path(), "phone", &backend.endpoint(), PHONE_KEY, &[]);
    let service = observing_with_home(&herdr, home.path()).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let mut client = connect(&service).await;
    add(&mut client, "w1:pA", "for the old chat", "00000000-0000-0000-0000-0000000000ff").await;
    add(&mut client, "w1:pB", "marker", PB_SESSION).await;

    let started = tokio::time::Instant::now();
    herdr.push_event(status("w1:pA", "idle"));
    herdr.push_event(status("w1:pB", "idle"));
    herdr.wait_until(|herdr| !prompts(herdr).is_empty()).await;

    // pA 的 guard 1 秒 + pB 的 guard 1 秒 ≈ 2 秒;推播若卡在迴圈裡會再多 1 秒以上。
    assert!(started.elapsed() < Duration::from_millis(2500), "pB 被 queue_paused 推播拖慢:{:?}", started.elapsed());
}
