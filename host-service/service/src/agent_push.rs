//! Agent 狀態類推播(raychiutw/purrdr.host#31,ADR-0004):可能卡住(`stuck`)、仍在等你(`reminder`)。
//!
//! 形狀與主機外掛相同(後台 `POST /v1/push`):只帶密鑰、workspace 名稱、狀態、pane id 與選用的 `tab`(done 另可帶選用的 `workSeconds`:工作秒數,算不出就省略,raychiutw/purrdr.host#96),不帶對話內容與終端機標題;
//! 密鑰只放在 HTTP 本文。靜音了這個聊天的裝置跳過，後台回 410 時刪掉那組配對(期間重新配對的不刪)。

use std::path::Path;

use crate::update_push::{PushTransport, pairings};

/// 後台：這支裝置的配對已撤銷。
const REVOKED: u16 = 410;

/// 對每支配對的裝置送一則;失敗不重試(每個狀態期間只送一次)。`tab` 是 Tab label 或 Agent 名稱,沒有就省略這個欄位。
#[allow(clippy::too_many_arguments)]
pub(crate) fn send(
    home: &Path,
    transport: &mut dyn PushTransport,
    status: &str,
    workspace: &str,
    pane_id: &str,
    session: Option<&str>,
    tab: Option<&str>,
    work_seconds: Option<u64>,
) {
    for pairing in pairings(&home.join(".purrdr")) {
        if session.is_some_and(|session| pairing.muted.iter().any(|muted| muted == session)) {
            continue;
        }
        let mut body = serde_json::json!({"key": pairing.key, "workspace": workspace, "status": status, "pane": pane_id});
        if let Some(tab) = tab {
            body["tab"] = serde_json::json!(tab);
        }
        if let Some(work_seconds) = work_seconds {
            body["workSeconds"] = serde_json::json!(work_seconds);
        }
        if transport.post_json(&pairing.endpoint, &body.to_string()) == Ok(REVOKED) {
            pairing.remove_if_unchanged();
        }
    }
}
