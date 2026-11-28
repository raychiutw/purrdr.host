//! 續接歷史 session(raychiutw/purrdr.host#32):列出工作資料夾過去的 Claude／Codex session,與確認 session 存在。
//!
//! 只讀對話紀錄檔的修改時間與開頭(找第一則使用者訊息當摘要),不回傳其他內容。

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::protocol::{HistorySession, ResumableAgentKind};
use crate::text_format::rfc3339;
use crate::transcript::{self, TranscriptEnvironment, TranscriptFailure};

/// 最多列出幾個 session。
const MAXIMUM_SESSIONS: usize = 50;
/// 摘要的字數上限。
const SUMMARY_CHARACTERS: usize = 120;
/// 找第一則使用者訊息時最多讀多少 bytes(Codex 的 session_meta 一行就可能很長)。
const SCAN_BYTES: u64 = 1024 * 1024;

/// session id:英數字與 `-`(與定位對話紀錄的規則相同),且不能以 `-` 開頭(會被 Agent 當成選項)。
pub(crate) fn is_safe_id(id: &str) -> bool {
    transcript::require_safe_id(id).is_ok() && !id.starts_with('-')
}

/// 這個資料夾過去的 session,依修改時間新到舊。
pub(crate) fn list(environment: &TranscriptEnvironment, cwd: &str, kind: ResumableAgentKind) -> Vec<HistorySession> {
    match kind {
        ResumableAgentKind::Claude => claude_sessions(environment, cwd),
        ResumableAgentKind::Codex => codex_sessions(environment, cwd),
    }
}

/// 這個 session 的對話紀錄是否存在(續接前確認;herdr 對不存在的 session 一樣回已啟動)。
pub(crate) fn exists(environment: &TranscriptEnvironment, kind: ResumableAgentKind, id: &str) -> bool {
    match kind {
        ResumableAgentKind::Claude => transcript::locate_claude(environment, id, "").is_file(),
        ResumableAgentKind::Codex => matches!(transcript::locate_codex(environment, id, None), Ok(_) | Err(TranscriptFailure::Ambiguous)),
    }
}

fn claude_sessions(environment: &TranscriptEnvironment, cwd: &str) -> Vec<HistorySession> {
    let folder = transcript::claude_projects(environment).join(transcript::claude_project_folder(cwd));
    let Ok(entries) = std::fs::read_dir(&folder) else {
        return Vec::new();
    };
    let mut files: Vec<(String, PathBuf, SystemTime)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let id = path.file_name()?.to_str()?.strip_suffix(".jsonl")?.to_owned();
            let metadata = entry.metadata().ok().filter(std::fs::Metadata::is_file)?;
            is_safe_id(&id).then(|| (id, path, metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
        })
        .collect();
    newest_first(&mut files);
    files
        .into_iter()
        .take(MAXIMUM_SESSIONS)
        .map(|(id, path, modified)| session(id, modified, first_line_matching(&path, claude_user_message)))
        .collect()
}

fn codex_sessions(environment: &TranscriptEnvironment, cwd: &str) -> Vec<HistorySession> {
    let mut paths = Vec::new();
    transcript::collect_files(
        &transcript::codex_home(environment).join("sessions"),
        transcript::CODEX_SEARCH_DEPTH,
        &mut paths,
    );
    let mut files: Vec<(String, PathBuf, SystemTime)> = paths
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
        })
        .map(|path| {
            let modified = std::fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            (String::new(), path, modified)
        })
        .collect();
    newest_first(&mut files);
    let mut sessions = Vec::new();
    for (_, path, modified) in files {
        if sessions.len() == MAXIMUM_SESSIONS {
            break;
        }
        let Some(meta) = first_line_matching(&path, |line| (line["type"] == "session_meta").then(|| line["payload"].clone())) else {
            continue;
        };
        let Some(id) = meta["id"].as_str().filter(|id| is_safe_id(id)) else {
            continue;
        };
        if meta["cwd"].as_str() == Some(cwd) {
            sessions.push(session(id.to_owned(), modified, first_line_matching(&path, codex_user_message)));
        }
    }
    sessions
}

fn newest_first(files: &mut [(String, PathBuf, SystemTime)]) {
    files.sort_by(|left, right| right.2.cmp(&left.2).then_with(|| left.1.cmp(&right.1)));
}

fn session(id: String, modified: SystemTime, message: Option<String>) -> HistorySession {
    HistorySession {
        id,
        modified: rfc3339(modified),
        summary: message.as_deref().map(summary).filter(|summary| !summary.is_empty()),
    }
}

/// 空白合併、截到 `SUMMARY_CHARACTERS` 字。
fn summary(message: &str) -> String {
    message
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(SUMMARY_CHARACTERS)
        .collect()
}

/// 檔案開頭(最多 `SCAN_BYTES`)第一個 `pick` 回 Some 的 JSON 行。
fn first_line_matching<T>(path: &Path, pick: impl Fn(&Value) -> Option<T>) -> Option<T> {
    let reader = BufReader::new(File::open(path).ok()?.take(SCAN_BYTES));
    reader
        .split(b'\n')
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_slice::<Value>(&line).ok())
        .find_map(|line| pick(&line))
}

/// Claude:使用者自己打的文字，略過 isMeta、`<…>` 開頭(指令與系統注入)、中斷提示與工具結果。
fn claude_user_message(line: &Value) -> Option<String> {
    if line["type"] != "user" || line["isMeta"] == true {
        return None;
    }
    let content = &line["message"]["content"];
    let text = content.as_str().or_else(|| {
        let first = content.as_array()?.first()?;
        (first["type"] == "text").then(|| first["text"].as_str()).flatten()
    })?;
    typed_by_user(text).then(|| text.to_owned())
}

/// Codex:`user_message` 事件，或使用者角色的 `input_text`(略過 `<…>` 開頭的環境與指示)。
fn codex_user_message(line: &Value) -> Option<String> {
    let payload = &line["payload"];
    let text = match (line["type"].as_str(), payload["type"].as_str()) {
        (Some("event_msg"), Some("user_message")) => payload["message"].as_str(),
        (Some("response_item"), Some("message")) if payload["role"] == "user" => {
            payload["content"].as_array()?.iter().find(|part| part["type"] == "input_text")?["text"].as_str()
        }
        _ => None,
    }?;
    typed_by_user(text).then(|| text.to_owned())
}

fn typed_by_user(text: &str) -> bool {
    let trimmed = text.trim_start();
    !trimmed.is_empty() && !trimmed.starts_with('<') && !trimmed.starts_with("[Request interrupted")
}
