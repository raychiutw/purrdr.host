//! 送出確認(raychiutw/purrdr.host#107):herdr 的 `agent.prompt` 只回報 `written_to_pty` 時，主機服務照 SSH 的規則
//! 等 agent 反應、必要時補一個 Enter,並用 `submit_confirmed`／`enter_sent` 回報結果。

mod support;

use serde_json::{Value, json};
use support::{FakeHerdr, TestClient, recorded, start_service};

/// `agent.prompt` 結果裡 agent 的 `state_change_seq`(fixture 的值),當作送出後的基準。
const PROMPT_SEQ: u64 = 1632;
const P6_SESSION: &str = "00000000-0000-0000-0000-000000000001";

fn hello() -> Value {
    json!({"type": "hello", "request_id": "r1", "protocol_version": 1})
}

fn prompt_request(extra: Value) -> Value {
    let mut request = json!({
        "type": "agent.prompt", "request_id": "r2", "pane_id": "w1:p6", "text": "next step",
        "wait": {"until": ["working", "blocked"], "timeout_ms": 8000},
    });
    for (key, value) in extra.as_object().unwrap() {
        request[key] = value.clone();
    }
    request
}

/// herdr 回 `written_to_pty` 的 agent.prompt 結果(fork 才有);`status`、`kind` 是 prompt 之後 agent 的狀態與種類。
fn written_prompt(status: &str, kind: &str) -> Value {
    let mut reply = recorded("agent.prompt.ndjson");
    reply["result"]["delivery"] = json!("written_to_pty");
    reply["result"]["agent"]["agent_status"] = json!(status);
    reply["result"]["agent"]["agent"] = json!(kind);
    reply
}

fn snapshot_with(status: &str, session: &str) -> Value {
    let mut snapshot = recorded("session.snapshot.ndjson");
    for agent in snapshot["result"]["snapshot"]["agents"].as_array_mut().unwrap() {
        if agent["pane_id"] == "w1:p6" {
            agent["agent_status"] = json!(status);
            agent["agent_session"]["value"] = json!(session);
            // 與 `written_prompt` 的基準相同:狀態沒轉換過。
            agent["state_change_seq"] = json!(PROMPT_SEQ);
        }
    }
    snapshot
}

fn wait_matched() -> Value {
    recorded("agent.wait.matched.ndjson")
}

fn wait_timeout() -> Value {
    recorded("error.agent-wait-timeout.ndjson")
}

async fn herdr(prompt: Value) -> FakeHerdr {
    FakeHerdr::start(vec![
        ("ping", recorded("ping.ndjson")),
        ("session.snapshot", snapshot_with("idle", P6_SESSION)),
        ("agent.prompt", prompt),
        ("agent.wait", wait_matched()),
        ("pane.send_keys", recorded("pane.send_keys.ndjson")),
    ])
    .await
}

async fn send(herdr: &FakeHerdr, request: Value) -> Value {
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    client.request(hello()).await;
    client.request(request).await
}

#[tokio::test]
async fn an_agent_that_starts_working_confirms_without_enter() {
    let herdr = herdr(written_prompt("idle", "claude")).await;

    let reply = send(&herdr, prompt_request(json!({}))).await;

    assert_eq!(reply["type"], "agent.prompt.result", "{reply}");
    assert_eq!(reply["submit_confirmed"], true);
    assert!(reply.get("enter_sent").is_none(), "{reply}");
    assert_eq!(herdr.methods(), ["ping", "agent.prompt", "agent.wait"]);
    let wait = &herdr.requests()[2]["params"];
    assert_eq!(wait["target"], "w1:p6");
    assert_eq!(wait["until"], json!(["working", "blocked"]));
    assert_eq!(wait["timeout_ms"], 3500);
}

#[tokio::test]
async fn no_reaction_sends_one_enter_and_confirms_when_it_reacts() {
    let herdr = herdr(written_prompt("idle", "claude")).await;
    herdr.reply_once("agent.wait", wait_timeout());

    let reply = send(&herdr, prompt_request(json!({"expected_session": P6_SESSION}))).await;

    assert_eq!(reply["submit_confirmed"], true, "{reply}");
    assert_eq!(reply["enter_sent"], true);
    assert_eq!(
        herdr.methods(),
        [
            "ping",
            "session.snapshot",
            "agent.prompt",
            "agent.wait",
            "session.snapshot",
            "pane.send_keys",
            "agent.wait"
        ]
    );
    assert_eq!(herdr.requests()[5]["params"], json!({"pane_id": "w1:p6", "keys": ["enter"]}));
    assert_eq!(herdr.requests()[6]["params"]["timeout_ms"], 2500);
}

#[tokio::test]
async fn still_no_reaction_after_enter_is_unconfirmed() {
    let herdr = herdr(written_prompt("done", "claude")).await;
    herdr.reply_once("agent.wait", wait_timeout());
    herdr.reply_once("agent.wait", wait_timeout());

    let reply = send(&herdr, prompt_request(json!({}))).await;

    assert_eq!(reply["type"], "agent.prompt.result", "{reply}");
    assert_eq!(reply["submit_confirmed"], false);
    assert_eq!(reply["enter_sent"], true);
}

#[tokio::test]
async fn enter_is_never_sent_to_a_blocked_agent() {
    let herdr = herdr(written_prompt("idle", "claude")).await;
    herdr.reply_once("agent.wait", wait_timeout());
    herdr.set_reply("session.snapshot", snapshot_with("blocked", P6_SESSION));

    let reply = send(&herdr, prompt_request(json!({}))).await;

    assert_eq!(reply["submit_confirmed"], false, "{reply}");
    assert!(reply.get("enter_sent").is_none(), "{reply}");
    assert!(!herdr.methods().contains(&"pane.send_keys".to_owned()));
}

#[tokio::test]
async fn enter_is_not_sent_when_the_session_changed_meanwhile() {
    let herdr = herdr(written_prompt("idle", "claude")).await;
    herdr.reply_once("agent.wait", wait_timeout());
    herdr.reply_once("session.snapshot", snapshot_with("idle", P6_SESSION));
    herdr.reply_once("session.snapshot", snapshot_with("idle", "00000000-0000-0000-0000-0000000000ff"));

    let reply = send(&herdr, prompt_request(json!({"expected_session": P6_SESSION}))).await;

    assert_eq!(reply["submit_confirmed"], false, "{reply}");
    assert!(reply.get("enter_sent").is_none(), "{reply}");
    assert!(!herdr.methods().contains(&"pane.send_keys".to_owned()));
}

#[tokio::test]
async fn an_agent_already_working_needs_no_waiting() {
    let herdr = herdr(written_prompt("working", "claude")).await;

    let reply = send(&herdr, prompt_request(json!({}))).await;

    assert_eq!(reply["submit_confirmed"], true, "{reply}");
    assert_eq!(herdr.methods(), ["ping", "agent.prompt"]);
}

#[tokio::test]
async fn nothing_extra_happens_when_the_prompt_was_not_only_written() {
    let herdr = herdr(recorded("agent.prompt.ndjson")).await;

    let reply = send(&herdr, prompt_request(json!({}))).await;

    assert_eq!(reply, json!({"type": "agent.prompt.result", "request_id": "r2"}));
    assert_eq!(herdr.methods(), ["ping", "agent.prompt"]);
}

#[tokio::test]
async fn commands_without_wait_are_left_alone() {
    let herdr = herdr(written_prompt("idle", "claude")).await;
    let mut request = prompt_request(json!({"text": "/compact"}));
    request.as_object_mut().unwrap().remove("wait");

    let reply = send(&herdr, request).await;

    assert_eq!(reply["delivery"], "written_to_pty", "{reply}");
    assert!(reply.get("submit_confirmed").is_none());
    assert_eq!(herdr.methods(), ["ping", "agent.prompt"]);
}

#[tokio::test]
async fn codex_has_no_receipt_so_nothing_is_confirmed_or_sent() {
    let herdr = herdr(written_prompt("idle", "codex")).await;

    let reply = send(&herdr, prompt_request(json!({}))).await;

    assert!(reply.get("submit_confirmed").is_none(), "{reply}");
    assert_eq!(herdr.methods(), ["ping", "agent.prompt"]);
}

#[tokio::test]
async fn an_agent_that_finished_before_the_wait_started_is_confirmed_without_enter() {
    let herdr = herdr(written_prompt("idle", "claude")).await;
    herdr.reply_once("agent.wait", wait_timeout());
    // 短 prompt 在等待開始前就 working 又回到 idle:state_change_seq 已前進。
    let mut snapshot = snapshot_with("idle", P6_SESSION);
    for agent in snapshot["result"]["snapshot"]["agents"].as_array_mut().unwrap() {
        if agent["pane_id"] == "w1:p6" {
            agent["state_change_seq"] = json!(PROMPT_SEQ + 2);
        }
    }
    herdr.set_reply("session.snapshot", snapshot);

    let reply = send(&herdr, prompt_request(json!({}))).await;

    assert_eq!(reply["submit_confirmed"], true, "{reply}");
    assert!(reply.get("enter_sent").is_none(), "{reply}");
    assert!(!herdr.methods().contains(&"pane.send_keys".to_owned()));
}

#[tokio::test]
async fn a_completion_since_the_prompt_also_counts_as_a_reaction() {
    let mut prompt = written_prompt("idle", "claude");
    prompt["result"]["agent"]["completion_seq"] = json!(10);
    let herdr = herdr(prompt).await;
    herdr.reply_once("agent.wait", wait_timeout());
    let mut snapshot = snapshot_with("done", P6_SESSION);
    for agent in snapshot["result"]["snapshot"]["agents"].as_array_mut().unwrap() {
        if agent["pane_id"] == "w1:p6" {
            agent["completion_seq"] = json!(11);
        }
    }
    herdr.set_reply("session.snapshot", snapshot);

    let reply = send(&herdr, prompt_request(json!({}))).await;

    assert_eq!(reply["submit_confirmed"], true, "{reply}");
    assert!(!herdr.methods().contains(&"pane.send_keys".to_owned()));
}
