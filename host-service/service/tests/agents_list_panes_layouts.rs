//! `agents.list` 回傳 panes 與 layouts(raychiutw/purrdr.host#103 範圍擴大):內容取自 herdr session.snapshot,
//! 讓 app 組出和 SSH 路線相同的 HostSnapshot(含純終端機 pane 與 Tab 內版面)。

mod support;

use serde_json::{Value, json};
use support::{FakeHerdr, TestClient, recorded, start_service};

async fn agents_list(snapshot: Value) -> Value {
    let herdr = FakeHerdr::start(vec![("ping", recorded("ping.ndjson")), ("session.snapshot", snapshot)]).await;
    let service = start_service(herdr.socket.clone()).await;
    let mut client = TestClient::connect(&service).await;
    client.request(json!({"type": "hello", "request_id": "r1", "protocol_version": 1})).await;
    client.request(json!({"type": "agents.list", "request_id": "r2"})).await
}

fn snapshot_reply(snapshot: Value) -> Value {
    json!({"id": "purrdr:1", "result": {"type": "session_snapshot", "snapshot": snapshot}})
}

#[tokio::test]
async fn panes_across_workspaces_and_tabs_carry_herdr_fields() {
    let reply = agents_list(snapshot_reply(json!({
        "workspaces": [{"workspace_id": "w1", "label": "garden"}, {"workspace_id": "w2", "label": "scratch"}],
        "tabs": [
            {"tab_id": "w1:t1", "workspace_id": "w1", "label": "a"},
            {"tab_id": "w1:t2", "workspace_id": "w1", "label": "b"},
            {"tab_id": "w2:t1", "workspace_id": "w2", "label": "c"}
        ],
        "panes": [
            {"pane_id": "w1:p1", "terminal_id": "term_1", "workspace_id": "w1", "tab_id": "w1:t1", "focused": true, "cwd": "/Users/demo/a",
             "foreground_cwd": "/Users/demo/a", "agent": "claude", "terminal_title": "✳ 整理", "terminal_title_stripped": "整理",
             "agent_status": "working", "agent_session": {"source": "herdr:claude", "agent": "claude", "kind": "id", "value": "s1"},
             "scroll": {"offset_from_bottom": 0}, "revision": 3},
            {"pane_id": "w1:p2", "workspace_id": "w1", "tab_id": "w1:t2", "focused": false, "agent_status": "idle", "agent": "codex"},
            {"pane_id": "w2:p1", "workspace_id": "w2", "tab_id": "w2:t1", "focused": false, "agent_status": "unknown"}
        ],
        "agents": []
    })))
    .await;

    assert_eq!(
        reply["panes"],
        json!([
            {"pane_id": "w1:p1", "terminal_id": "term_1", "workspace_id": "w1", "tab_id": "w1:t1", "focused": true, "cwd": "/Users/demo/a",
             "foreground_cwd": "/Users/demo/a", "agent": "claude", "terminal_title": "✳ 整理", "terminal_title_stripped": "整理",
             "agent_status": "working", "agent_session": {"source": "herdr:claude", "agent": "claude", "kind": "id", "value": "s1"}},
            {"pane_id": "w1:p2", "workspace_id": "w1", "tab_id": "w1:t2", "focused": false, "agent": "codex", "agent_status": "idle"},
            {"pane_id": "w2:p1", "workspace_id": "w2", "tab_id": "w2:t1", "focused": false, "agent_status": "unknown"}
        ])
    );
}

#[tokio::test]
async fn a_plain_terminal_pane_has_no_agent_and_is_not_in_agents() {
    let reply = agents_list(snapshot_reply(json!({
        "panes": [{"pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1", "focused": true, "cwd": "/Users/demo/x", "agent_status": "unknown"}],
        "agents": []
    })))
    .await;

    assert_eq!(reply["agents"], json!([]));
    let pane = &reply["panes"][0];
    assert_eq!(pane["pane_id"], "w1:p1");
    assert_eq!(pane["agent_status"], "unknown");
    assert!(pane.get("agent").is_none());
    assert!(pane.get("agent_session").is_none());
}

#[tokio::test]
async fn a_pane_without_status_is_unknown() {
    let reply = agents_list(snapshot_reply(
        json!({"panes": [{"pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1"}]}),
    ))
    .await;

    assert_eq!(reply["panes"][0]["agent_status"], "unknown");
    assert_eq!(reply["panes"][0]["focused"], false);
}

#[tokio::test]
async fn layouts_carry_area_panes_and_splits() {
    let reply = agents_list(snapshot_reply(json!({"layouts": [
        {"workspace_id": "w1", "tab_id": "w1:t1", "zoomed": true, "area": {"x": 0, "y": 0, "width": 120, "height": 40}, "focused_pane_id": "w1:p1",
         "panes": [{"pane_id": "w1:p1", "focused": true, "rect": {"x": 0, "y": 0, "width": 60, "height": 40}},
                   {"pane_id": "w1:p2", "focused": false, "rect": {"x": 60, "y": 0, "width": 60, "height": 40}}],
         "splits": [{"id": "split_0_root", "direction": "right", "ratio": 0.5, "rect": {"x": 0, "y": 0, "width": 120, "height": 40}}]},
        {"workspace_id": "w2", "tab_id": "w2:t1", "zoomed": false, "area": {"x": 0, "y": 0, "width": 120, "height": 40}, "focused_pane_id": "w2:p1",
         "panes": [{"pane_id": "w2:p1", "focused": true, "rect": {"x": 0, "y": 0, "width": 120, "height": 40}}], "splits": []}
    ]})))
    .await;

    let layouts = reply["layouts"].as_array().unwrap();
    assert_eq!(layouts.len(), 2);
    assert_eq!(layouts[0]["zoomed"], true);
    assert_eq!(layouts[0]["focused_pane_id"], "w1:p1");
    assert_eq!(layouts[0]["area"], json!({"x": 0, "y": 0, "width": 120, "height": 40}));
    assert_eq!(
        layouts[0]["panes"][1],
        json!({"pane_id": "w1:p2", "focused": false, "rect": {"x": 60, "y": 0, "width": 60, "height": 40}})
    );
    assert_eq!(
        layouts[0]["splits"],
        json!([{"id": "split_0_root", "direction": "right", "ratio": 0.5, "rect": {"x": 0, "y": 0, "width": 120, "height": 40}}])
    );
    assert_eq!(layouts[1]["splits"], json!([]));
}

#[tokio::test]
async fn the_recorded_split_snapshot_keeps_nested_splits_and_pane_rects() {
    let reply = agents_list(recorded("session.snapshot.split.ndjson")).await;

    let panes = reply["panes"].as_array().unwrap();
    assert_eq!(panes.len(), 4);
    assert!(panes.iter().all(|pane| pane["agent_status"] == "unknown" && pane.get("agent").is_none()));

    let layouts = reply["layouts"].as_array().unwrap();
    assert_eq!(layouts.len(), 2);
    let split_tab = &layouts[0];
    assert_eq!(split_tab["tab_id"], "w1:t1");
    assert_eq!(split_tab["panes"].as_array().unwrap().len(), 3);
    let splits = split_tab["splits"].as_array().unwrap();
    assert_eq!(splits.len(), 2);
    assert_eq!(splits[0]["id"], "split_0_root");
    assert_eq!(splits[0]["direction"], "right");
    assert_eq!(splits[0]["ratio"], 0.4);
    assert_eq!(splits[1]["direction"], "down");
    assert_eq!(splits[1]["rect"], json!({"x": 48, "y": 0, "width": 72, "height": 40}));
    assert_eq!(layouts[1]["splits"], json!([]));
}

#[tokio::test]
async fn the_recorded_snapshot_without_splits_lists_panes_and_layouts() {
    let reply = agents_list(recorded("session.snapshot.ndjson")).await;

    assert_eq!(reply["panes"].as_array().unwrap().len(), 4);
    assert_eq!(reply["layouts"].as_array().unwrap().len(), 4);
    let terminal = reply["panes"].as_array().unwrap().iter().find(|pane| pane["pane_id"] == "w2:p1").unwrap();
    assert!(terminal.get("agent").is_none());
}

#[tokio::test]
async fn a_snapshot_without_panes_or_layouts_gives_empty_lists() {
    let reply = agents_list(snapshot_reply(json!({"workspaces": [], "agents": []}))).await;

    assert_eq!(reply["panes"], json!([]));
    assert_eq!(reply["layouts"], json!([]));
}
