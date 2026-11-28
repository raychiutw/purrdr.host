//! 一條 app 連線上的對話紀錄操作(#59):記住這條連線定位過的路徑，讀取只接受這些路徑與其 subagent 對話紀錄。

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::connection::error;
use crate::http::blocking;
use crate::protocol::{
    ErrorCode, ServerMessage, TranscriptDelegation, TranscriptDelegationResult, TranscriptFollow, TranscriptFollowResult, TranscriptLocateResult,
    TranscriptPreview, TranscriptPreviews, TranscriptPreviewsResult, TranscriptRecord, TranscriptStreamEnd, TranscriptSubagent, TranscriptSubagentResult,
    TranscriptUnfollow, TranscriptUnfollowResult, TranscriptUpdate, TranscriptWindow, TranscriptWindowResult, TranscriptWorkflow, TranscriptWorkflowResult,
};
use crate::transcript::{self, AgentSessionInfo, LongLineScan, Record, Tail, TranscriptEnvironment, TranscriptFailure};
use crate::transcript_reads;

/// 追蹤中的對話紀錄多久檢查一次(主機本機的檔案大小，不經 herdr)。
const FOLLOW_INTERVAL: Duration = Duration::from_millis(250);

pub(crate) struct TranscriptSession {
    environment: TranscriptEnvironment,
    located: HashSet<String>,
    /// 追蹤中的串流;連線結束(drop)時全部停止。
    streams: HashMap<String, JoinHandle<()>>,
    /// 推送給 app 的訊息。
    pushes: UnboundedSender<ServerMessage>,
}

impl Drop for TranscriptSession {
    fn drop(&mut self) {
        for stream in self.streams.values() {
            stream.abort();
        }
    }
}

impl TranscriptSession {
    pub fn new(environment: TranscriptEnvironment, pushes: UnboundedSender<ServerMessage>) -> Self {
        TranscriptSession {
            environment,
            located: HashSet::new(),
            streams: HashMap::new(),
            pushes,
        }
    }

    /// 開始追蹤：回應帶游標之後已有的完整行，之後新增的行以 `transcript.update` 推送。
    pub async fn follow(&mut self, request: TranscriptFollow) -> ServerMessage {
        if !self.is_readable(&request.path) {
            return failure_message(request.request_id, TranscriptFailure::NotAllowed);
        }
        if self.streams.contains_key(&request.stream_id) {
            return invalid(request.request_id, "stream_id is already in use");
        }
        let (Ok(from), Ok(limit)) = (u64::try_from(request.from), u64::try_from(request.limit)) else {
            return invalid(request.request_id, "from and limit must not be negative");
        };
        let path = PathBuf::from(&request.path);
        let reading = path.clone();
        let tail = blocking(move || {
            let anchor = transcript::anchor(&reading, from)?;
            let mut scan = LongLineScan::default();
            transcript::tail(&reading, from, limit, &anchor, &mut scan).map(|tail| (tail, scan))
        })
        .await;
        let (tail, scan) = match tail {
            Ok(tail) => tail,
            Err(failure) => return failure_message(request.request_id, failure),
        };
        let update = StreamUpdate::from_tail(from, tail);
        if update.ended.is_none() {
            let follow = Following {
                cursor: update.cursor,
                anchor: update.anchor.clone(),
                scan,
            };
            let task = tokio::spawn(track(path, follow, limit, request.stream_id.clone(), self.pushes.clone()));
            self.streams.insert(request.stream_id.clone(), task);
        }
        ServerMessage::TranscriptFollowResult(TranscriptFollowResult {
            request_id: request.request_id,
            stream_id: request.stream_id,
            size: update.size as i64,
            start: from as i64,
            records: protocol_records(update.records),
            ended: update.ended,
        })
    }

    /// 停止追蹤(不認得的串流也回成功)。
    pub fn unfollow(&mut self, request: TranscriptUnfollow) -> ServerMessage {
        if let Some(stream) = self.streams.remove(&request.stream_id) {
            stream.abort();
        }
        ServerMessage::TranscriptUnfollowResult(TranscriptUnfollowResult {
            request_id: request.request_id,
        })
    }

    /// 定位並取得大小;成功後這個路徑才可讀。
    pub async fn locate(&mut self, request_id: String, session: AgentSessionInfo, header: bool) -> ServerMessage {
        let environment = self.environment.clone();
        let located = blocking(move || {
            let path = transcript::locate(&environment, &session)?;
            let size = transcript::probe(&path)?;
            let header = if header { Some(transcript::header_lines(&path)?) } else { None };
            Ok::<_, TranscriptFailure>((path, size, header))
        })
        .await;
        match located {
            Ok((path, size, header)) => {
                let path = path.to_string_lossy().into_owned();
                self.located.insert(path.clone());
                ServerMessage::TranscriptLocateResult(TranscriptLocateResult {
                    request_id,
                    path,
                    size: size as i64,
                    header,
                })
            }
            Err(failure) => failure_message(request_id, failure),
        }
    }

    /// 這條連線定位過的對話紀錄(主機取檔以其中出現的路徑放行)。
    pub fn located_paths(&self) -> Vec<PathBuf> {
        self.located.iter().map(PathBuf::from).collect()
    }

    /// 讀視窗;路徑必須是這條連線可讀的(`is_readable`)。
    pub async fn window(&self, request: TranscriptWindow) -> ServerMessage {
        if !self.is_readable(&request.path) {
            return failure_message(request.request_id, TranscriptFailure::NotAllowed);
        }
        let (Ok(end), Ok(lines)) = (u64::try_from(request.end), usize::try_from(request.lines)) else {
            return invalid(request.request_id, "end and lines must not be negative");
        };
        if lines == 0 {
            return invalid(request.request_id, "lines must be positive");
        }
        let max_bytes = match request.max_bytes.map(u64::try_from) {
            None => None,
            Some(Ok(max_bytes)) if max_bytes > 0 => Some(max_bytes),
            Some(_) => return invalid(request.request_id, "max_bytes must be positive"),
        };
        let path = PathBuf::from(&request.path);
        match blocking(move || transcript::window(&path, end, lines, max_bytes)).await {
            Ok(chunk) => ServerMessage::TranscriptWindowResult(TranscriptWindowResult {
                request_id: request.request_id,
                start: chunk.start as i64,
                end: chunk.end as i64,
                records: protocol_records(chunk.records),
            }),
            Err(failure) => failure_message(request.request_id, failure),
        }
    }

    /// 這條連線可讀的對話紀錄：定位過的，或其 session 資料夾 `subagents/` 底下的 `.jsonl`(subagent、workflow agent)。
    fn is_readable(&self, path: &str) -> bool {
        if self.located.contains(path) {
            return true;
        }
        let requested = Path::new(path);
        let lexically_safe = path.ends_with(".jsonl") && !requested.components().any(|component| component == Component::ParentDir);
        lexically_safe
            && self
                .located
                .iter()
                .any(|located| requested.starts_with(transcript_reads::subagents_folder(Path::new(located))))
    }

    /// 列表預覽;每個路徑都必須是這條連線可讀的。讀不到的不在結果裡。
    pub async fn previews(&self, request: TranscriptPreviews) -> ServerMessage {
        if request.entries.iter().any(|entry| !self.is_readable(&entry.path)) {
            return failure_message(request.request_id, TranscriptFailure::NotAllowed);
        }
        let entries = request.entries;
        let previews = blocking(move || {
            entries
                .into_iter()
                .filter_map(|entry| {
                    let preview = transcript_reads::preview(Path::new(&entry.path), entry.known_version.as_deref()).ok()?;
                    Some(TranscriptPreview {
                        key: entry.key,
                        change: preview.change,
                        version: preview.version,
                        records: protocol_records(preview.records),
                    })
                })
                .collect()
        })
        .await;
        ServerMessage::TranscriptPreviewsResult(TranscriptPreviewsResult {
            request_id: request.request_id,
            previews,
        })
    }

    /// 找呼叫 id 啟動的 subagent 對話紀錄;路徑必須是這條連線可讀的。
    pub async fn subagent(&self, request: TranscriptSubagent) -> ServerMessage {
        if !self.is_readable(&request.path) {
            return failure_message(request.request_id, TranscriptFailure::NotAllowed);
        }
        if !transcript_reads::is_safe_delegation_id(&request.call_id) {
            return invalid(request.request_id, "call_id has unsafe characters");
        }
        let parent = PathBuf::from(&request.path);
        match blocking(move || transcript_reads::find_subagent(&parent, &request.call_id)).await {
            Ok(path) => ServerMessage::TranscriptSubagentResult(TranscriptSubagentResult {
                request_id: request.request_id,
                path: path.to_string_lossy().into_owned(),
            }),
            Err(failure) => failure_message(request.request_id, failure),
        }
    }

    /// 讀 workflow 進度;路徑必須是這條連線可讀的。
    pub async fn workflow(&self, request: TranscriptWorkflow) -> ServerMessage {
        if !self.is_readable(&request.path) {
            return failure_message(request.request_id, TranscriptFailure::NotAllowed);
        }
        if !transcript_reads::is_safe_delegation_id(&request.run_id) {
            return invalid(request.request_id, "run_id has unsafe characters");
        }
        let transcript = PathBuf::from(&request.path);
        let (run_id, known_checksum) = (request.run_id, request.known_checksum);
        match blocking(move || transcript_reads::workflow(&transcript, &run_id, known_checksum.as_deref())).await {
            Ok(progress) => ServerMessage::TranscriptWorkflowResult(TranscriptWorkflowResult {
                request_id: request.request_id,
                kind: progress.kind,
                checksum: progress.checksum,
                content: progress.content,
            }),
            Err(failure) => failure_message(request.request_id, failure),
        }
    }

    /// 父對話紀錄中指名委派的行;路徑必須是這條連線可讀的。
    pub async fn delegation(&self, request: TranscriptDelegation) -> ServerMessage {
        if !self.is_readable(&request.path) {
            return failure_message(request.request_id, TranscriptFailure::NotAllowed);
        }
        let agent_id = request.agent_id.unwrap_or_else(|| request.call_id.clone());
        if ![&request.call_id, &agent_id].iter().all(|id| transcript_reads::is_safe_delegation_id(id)) {
            return invalid(request.request_id, "call_id and agent_id must have only safe characters");
        }
        let transcript = PathBuf::from(&request.path);
        let (call_id, known_checksum) = (request.call_id, request.known_checksum);
        let lines = blocking(move || transcript_reads::delegation_lines(&transcript, &[&call_id, &agent_id], known_checksum.as_deref())).await;
        match lines {
            Ok(lines) => ServerMessage::TranscriptDelegationResult(TranscriptDelegationResult {
                request_id: request.request_id,
                checksum: lines.checksum,
                records: lines.records.map(protocol_records),
            }),
            Err(failure) => failure_message(request.request_id, failure),
        }
    }
}

/// 一次檢查的結果。
struct StreamUpdate {
    size: u64,
    records: Vec<Record>,
    /// 下一次從這裡讀。
    cursor: u64,
    /// 下一次讀時比對的游標前內容。
    anchor: Vec<u8>,
    ended: Option<TranscriptStreamEnd>,
}

impl StreamUpdate {
    fn from_tail(from: u64, tail: Tail) -> Self {
        let ended = |size, reason| StreamUpdate {
            size,
            records: Vec::new(),
            cursor: from,
            anchor: Vec::new(),
            ended: Some(reason),
        };
        match tail {
            Tail::Lines { size, chunk, anchor } => StreamUpdate {
                size,
                records: chunk.records,
                cursor: chunk.end,
                anchor,
                ended: None,
            },
            Tail::Shrank { size } => ended(size, TranscriptStreamEnd::Shrank),
            Tail::Replaced { size } => ended(size, TranscriptStreamEnd::Replaced),
        }
    }

    fn from_failure(from: u64, failure: TranscriptFailure) -> Self {
        let reason = match failure {
            TranscriptFailure::Absent => TranscriptStreamEnd::Absent,
            _ => TranscriptStreamEnd::Unreadable,
        };
        StreamUpdate {
            size: 0,
            records: Vec::new(),
            cursor: from,
            anchor: Vec::new(),
            ended: Some(reason),
        }
    }
}

/// 追蹤中的一條串流：游標、比對換檔用的游標前內容，與超長行的掃描進度。
struct Following {
    cursor: u64,
    anchor: Vec<u8>,
    scan: LongLineScan,
}

/// 定期檢查檔案，有新的完整行就推送(每次最多 `limit` bytes,落後很多時分段接著送);檔案變小、被換掉、讀不到時推送結束原因後停止。
async fn track(path: PathBuf, mut follow: Following, limit: u64, stream_id: String, pushes: UnboundedSender<ServerMessage>) {
    loop {
        tokio::time::sleep(FOLLOW_INTERVAL).await;
        let reading = path.clone();
        let from = follow.cursor;
        // 超長行還沒寫完時的掃描進度跟著送進去再拿回來，下一次從掃描到的位置接著找換行。
        let (result, scan) = blocking(move || {
            let mut scan = follow.scan;
            let result = transcript::tail(&reading, from, limit, &follow.anchor, &mut scan);
            (result.map(|tail| (tail, follow.anchor)), scan)
        })
        .await;
        let (update, anchor) = match result {
            Ok((tail, anchor)) => (StreamUpdate::from_tail(from, tail), anchor),
            Err(failure) => (StreamUpdate::from_failure(from, failure), Vec::new()),
        };
        if update.ended.is_none() && update.records.is_empty() {
            follow = Following { cursor: from, anchor, scan };
            continue;
        }
        let ended = update.ended.is_some();
        follow = Following {
            cursor: update.cursor,
            anchor: update.anchor,
            scan,
        };
        let message = ServerMessage::TranscriptUpdate(TranscriptUpdate {
            stream_id: stream_id.clone(),
            size: update.size as i64,
            start: from as i64,
            records: protocol_records(update.records),
            ended: update.ended,
        });
        if pushes.send(message).is_err() || ended {
            return;
        }
    }
}

fn protocol_records(records: Vec<Record>) -> Vec<TranscriptRecord> {
    records
        .into_iter()
        .map(|record| TranscriptRecord {
            length: record.length as i64,
            text: record.text,
        })
        .collect()
}

fn invalid(request_id: String, message: &str) -> ServerMessage {
    error(Some(request_id), ErrorCode::InvalidMessage, message)
}

fn failure_message(request_id: String, failure: TranscriptFailure) -> ServerMessage {
    let (code, message) = match failure {
        TranscriptFailure::Absent => (ErrorCode::TranscriptAbsent, "transcript does not exist yet"),
        TranscriptFailure::Unreadable => (ErrorCode::TranscriptUnreadable, "transcript is not readable"),
        TranscriptFailure::Ambiguous => (ErrorCode::TranscriptAmbiguous, "more than one transcript matches the session"),
        TranscriptFailure::Compressed => (ErrorCode::TranscriptCompressed, "only a compressed transcript exists"),
        TranscriptFailure::NotAllowed => (ErrorCode::TranscriptNotAllowed, "session reference is not allowed"),
        TranscriptFailure::Unsupported => (ErrorCode::TranscriptUnsupported, "host service cannot read this agent's transcript"),
        TranscriptFailure::TooLarge => (
            ErrorCode::TranscriptTooLarge,
            "transcript is too large to read that many lines; pass max_bytes or ask for fewer lines",
        ),
    };
    error(Some(request_id), code, message)
}
