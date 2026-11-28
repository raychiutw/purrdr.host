//! `agents.list` 回傳 Tab 名稱(raychiutw/purrdr.host#103):內容取自 herdr session.snapshot 的 `tabs`,與 SSH 路線看到的一致。

mod support;

use serde_json::{Value, json};
use support::{FakeHerdr, TestClient, recorded, start_service};

fn snapshot_reply(tabs: Value) -> Value {
    json!({"id": "purrdr:1", "result": {"type": "session_snapshot", "snapshot": {
        "workspaces": [{"workspace_id": "w1", "label": "garden"}, {"workspace_id": "w2", "label": "scratch"}],
        "tabs": tabs,
        "agents": []
    }}})
}

async fn agents_list(snapshot: Value) -> Value {
    let herdr = FakeHerdr::start(vec![("ping", recorded("ping.ndjson")), ("session.snapshot", snapshot)]).await;
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    client.request(json!({"type": "hello", "request_id": "r1", "protocol_version": 1})).await;
    client.request(json!({"type": "agents.list", "request_id": "r2"})).await
}

#[tokio::test]
async fn tabs_across_workspaces_carry_id_workspace_and_label() {
    let reply = agents_list(snapshot_reply(json!([
        {"tab_id": "w1:t1", "workspace_id": "w1", "number": 1, "label": "purrdr.ios", "focused": true, "pane_count": 1, "agent_status": "idle"},
        {"tab_id": "w1:t2", "workspace_id": "w1", "number": 2, "label": "purrdr.host", "focused": false, "pane_count": 1, "agent_status": "working"},
        {"tab_id": "w2:t1", "workspace_id": "w2", "number": 1, "label": "build-logs", "focused": false, "pane_count": 1, "agent_status": "unknown"}
    ])))
    .await;

    assert_eq!(
        reply["tabs"],
        json!([
            {"tab_id": "w1:t1", "workspace_id": "w1", "label": "purrdr.ios"},
            {"tab_id": "w1:t2", "workspace_id": "w1", "label": "purrdr.host"},
            {"tab_id": "w2:t1", "workspace_id": "w2", "label": "build-logs"}
        ])
    );
}

#[tokio::test]
async fn a_tab_without_a_label_is_listed_with_an_empty_label() {
    let reply = agents_list(snapshot_reply(json!([
        {"tab_id": "w1:t1", "workspace_id": "w1"},
        {"tab_id": "w1:t2", "workspace_id": "w1", "label": null}
    ])))
    .await;

    assert_eq!(
        reply["tabs"],
        json!([
            {"tab_id": "w1:t1", "workspace_id": "w1", "label": ""},
            {"tab_id": "w1:t2", "workspace_id": "w1", "label": ""}
        ])
    );
}

#[tokio::test]
async fn a_snapshot_without_tabs_gives_an_empty_list() {
    let reply = agents_list(json!({"id": "purrdr:1", "result": {"type": "session_snapshot", "snapshot": {"workspaces": [], "agents": []}}})).await;

    assert_eq!(reply["tabs"], json!([]));
}

#[tokio::test]
async fn the_recorded_snapshot_lists_its_tabs() {
    let reply = agents_list(recorded("session.snapshot.ndjson")).await;

    let tabs = reply["tabs"].as_array().unwrap();
    assert_eq!(tabs.len(), 4);
    assert_eq!(tabs[0], json!({"tab_id": "w1:t6", "workspace_id": "w1", "label": "1"}));
}
