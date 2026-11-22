//! Claude 方案用量與每個 session 的 context(raychiutw/purrdr.host#62、#64)。
//!
//! 安裝與還原 Claude Code 的 status line 包裝:與 app 經 SSH 的 `ClaudePlanUsageHost`／`ReversibleHostWrite`
//! (purrdr.ios,ADR-0002)同一套規則與檔案,所以兩條路徑可以互相關閉:
//! - 包裝腳本與原本的指令放在 `~/.purrdr/claude-usage/`(`statusline.sh`／`statusline.ps1`、`original-command`);
//! - `journal.json` 是 app 的 `HostWriteJournal`(`version`、`entries[{path, original, installed}]`、
//!   `createdDirectories`;`original`／`installed` 是標準 base64,`path` 相對家目錄);
//! - 還原時設定檔仍是安裝寫入的樣子 → 換回安裝前的 bytes,否則只撤掉 `statusLine` 的修改(merge)。
//!
//! 讀取:`usage.json`(整台主機的方案用量)與 `sessions/<session_id>.json`(每個 session 的 context),
//! 腳本寫的格式見 `claude-usage/statusline.sh`。

use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value, json};

use crate::connection::error;
use crate::protocol::{
    ClaudeUsageInstallResult, ClaudeUsageRemoveResult, ClaudeUsageStatusResult, ClientMessage, ErrorCode, PlanUsagePeriod, PlanUsagePeriodKind, ServerMessage,
    SessionContext, UsageClaudeResult, UsageSessionContextResult,
};

/// 包裝腳本(單一來源在 repo 的 `claude-usage/`,app 以 sync 取回同一份)。
const SHELL_WRAPPER: &str = include_str!("../../../claude-usage/statusline.sh");
const POWERSHELL_WRAPPER: &str = include_str!("../../../claude-usage/statusline.ps1");
/// 寫進 `statusLine.command` 的指令(Claude Code 以 shell 執行，`~` 會展開;同 app 的 `wrapperCommand`)。
const SHELL_COMMAND: &str = "~/.purrdr/claude-usage/statusline.sh";
const POWERSHELL_SCRIPT: &str = ".purrdr/claude-usage/statusline.ps1";
const FEATURE_DIRECTORY: &str = ".purrdr/claude-usage";
/// 單一用量檔的大小上限(status line 輸入通常不到 10 KB)。
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// `usage.session_context` 一次最多的 session 數。
pub(crate) const MAX_SESSIONS: usize = 200;

/// 同一時間只處理一個安裝或還原(兩支裝置同時按時不會交錯)。
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 包裝目前的狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClaudeUsageState {
    pub installed: bool,
    pub wrapper_active: bool,
    pub claude_found: bool,
}

pub(crate) struct PlanUsage {
    pub installed: bool,
    pub captured_at: Option<String>,
    pub periods: Vec<PlanUsagePeriod>,
}

#[derive(Debug)]
pub(crate) enum ClaudeUsageFailure {
    InvalidRequest(&'static str),
    /// Claude Code 的設定資料夾不存在。
    ClaudeNotFound,
    /// 設定檔不是(嚴格的)JSON 物件。
    SettingsNotJson,
    /// 檔案不能安全地修改(安裝紀錄壞了、不是一般檔案、讀取後被改過…)。
    WriteFailed(String),
    Internal(String),
}

impl From<std::io::Error> for ClaudeUsageFailure {
    fn from(error: std::io::Error) -> Self {
        ClaudeUsageFailure::WriteFailed(error.to_string())
    }
}

// MARK: - 安裝紀錄(app 的 HostWriteJournal)

/// `~/.purrdr/claude-usage/journal.json`。欄位順序同 Swift `JSONEncoder(sortedKeys)` 的輸出。
#[derive(Debug, Serialize, Deserialize)]
struct Journal {
    #[serde(rename = "createdDirectories", default, skip_serializing_if = "Option::is_none")]
    created_directories: Option<Vec<String>>,
    entries: Vec<JournalEntry>,
    #[serde(default = "journal_version")]
    version: i64,
}

fn journal_version() -> i64 {
    1
}

#[derive(Debug, Serialize, Deserialize)]
struct JournalEntry {
    #[serde(with = "base64_bytes")]
    installed: Vec<u8>,
    /// 安裝前的內容;沒有表示原本不存在。
    #[serde(default, skip_serializing_if = "Option::is_none", with = "optional_base64_bytes")]
    original: Option<Vec<u8>>,
    path: String,
}

mod base64_bytes {
    use super::*;

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}

mod optional_base64_bytes {
    use super::*;

    pub fn serialize<S: Serializer>(bytes: &Option<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => serializer.serialize_str(&STANDARD.encode(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| STANDARD.decode(text).map_err(serde::de::Error::custom))
            .transpose()
    }
}

// MARK: - 檔案狀態

/// 檔案讀到的樣子(同 app 的 `HostFileState`)。
#[derive(Debug, Clone, PartialEq, Eq)]
enum FileState {
    Missing,
    /// 檔案所在的資料夾不存在。
    MissingDirectory,
    File(Vec<u8>),
    /// 不是一般檔案或讀不到。
    Unusable,
}

impl FileState {
    fn read(path: &Path) -> FileState {
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_file() && metadata.len() <= 16 * MAX_FILE_BYTES => match fs::read(path) {
                Ok(bytes) => FileState::File(bytes),
                Err(_) => FileState::Unusable,
            },
            Ok(_) => FileState::Unusable,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                if path.parent().is_some_and(Path::is_dir) {
                    FileState::Missing
                } else {
                    FileState::MissingDirectory
                }
            }
            Err(_) => FileState::Unusable,
        }
    }

    /// 檔案現有的內容(不存在是 `None`)。
    fn contents(&self) -> Option<&[u8]> {
        match self {
            FileState::File(bytes) => Some(bytes),
            _ => None,
        }
    }
}

// MARK: - 設定檔

/// 只接受 Claude Code 也讀得了的嚴格 JSON 物件。
fn json_object(bytes: &[u8]) -> Option<Map<String, Value>> {
    match serde_json::from_slice::<Value>(bytes).ok()? {
        Value::Object(object) => Some(object),
        _ => None,
    }
}

fn status_line_command(settings: Option<&[u8]>) -> Option<String> {
    let object = json_object(settings?)?;
    Some(object.get("statusLine")?.get("command")?.as_str()?.to_owned())
}

/// 是 Purrdr 的包裝(任一種主機的;同 app 的 `isWrapper`)。
fn is_wrapper(command: Option<&str>) -> bool {
    let Some(command) = command else { return false };
    command == SHELL_COMMAND || (command.starts_with("powershell.exe ") && command.ends_with(&format!("/{POWERSHELL_SCRIPT}'")))
}

/// 排版同 Foundation `JSONSerialization`(prettyPrinted、sortedKeys):兩格縮排、`"鍵" : 值`、空容器 `{\n\n}`;
/// 結尾換行。app 與主機服務寫出的設定檔因此排版一致。
fn render_settings(object: &Map<String, Value>) -> Vec<u8> {
    let mut output = String::new();
    render(&Value::Object(object.clone()), 0, &mut output);
    output.push('\n');
    output.into_bytes()
}

fn render(value: &Value, depth: usize, output: &mut String) {
    let padding = "  ".repeat(depth);
    match value {
        Value::Object(object) if object.is_empty() => output.push_str(&format!("{{\n\n{padding}}}")),
        Value::Array(items) if items.is_empty() => output.push_str(&format!("[\n\n{padding}]")),
        Value::Object(object) => {
            output.push_str("{\n");
            // 鍵依字母排序(同 sortedKeys;不依賴 serde_json 的 Map 順序，preserve_order 開著時是插入順序)。
            let mut entries: Vec<(&String, &Value)> = object.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    output.push_str(",\n");
                }
                output.push_str(&format!("{padding}  {} : ", Value::String(key.clone())));
                render(item, depth + 1, output);
            }
            output.push_str(&format!("\n{padding}}}"));
        }
        Value::Array(items) => {
            output.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    output.push_str(",\n");
                }
                output.push_str(&format!("{padding}  "));
                render(item, depth + 1, output);
            }
            output.push_str(&format!("\n{padding}]"));
        }
        scalar => output.push_str(&scalar.to_string()),
    }
}

// MARK: - 寫檔

/// 以暫存檔寫好再換上(讀的一方不會讀到寫一半的檔);已存在的檔案沿用原本的權限,symlink 寫到它指向的檔案。
fn write_atomically(path: &Path, contents: &[u8], executable: bool) -> std::io::Result<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if let Some(directory) = target.parent() {
        fs::create_dir_all(directory)?;
    }
    let name = target.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = target.with_file_name(format!(".{name}.purrdr-tmp"));
    let written = (|| {
        fs::write(&temporary, contents)?;
        if let Ok(existing) = fs::metadata(&target) {
            fs::set_permissions(&temporary, existing.permissions())?;
        } else if executable {
            set_executable(&temporary)?;
        }
        fs::rename(&temporary, &target)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

#[cfg(unix)]
fn set_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// 讀取之後檔案沒被改過才寫(compare-and-swap;同 app 的 `expecting: .version`):`expected` 是讀到的內容。
fn write_unless_changed(path: &Path, expected: &FileState, contents: &[u8]) -> Result<(), ClaudeUsageFailure> {
    if &FileState::read(path) != expected {
        return Err(changed(path));
    }
    Ok(write_atomically(path, contents, false)?)
}

fn changed(path: &Path) -> ClaudeUsageFailure {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    ClaudeUsageFailure::WriteFailed(format!("{name} changed since it was read"))
}

// MARK: - 主機端

#[derive(Debug, Clone)]
pub(crate) struct ClaudeUsage {
    home: Option<PathBuf>,
    claude_config_dir: Option<PathBuf>,
    /// Windows 主機:包裝是 PowerShell 版。
    windows: bool,
}

/// 安裝或還原前讀到的設定檔與安裝紀錄。
struct Inspection {
    settings: FileState,
    journal: Option<Journal>,
}

impl ClaudeUsage {
    pub fn new(home: Option<PathBuf>, claude_config_dir: Option<PathBuf>) -> Self {
        ClaudeUsage {
            home,
            claude_config_dir,
            windows: cfg!(windows),
        }
    }

    fn home(&self) -> Result<&Path, ClaudeUsageFailure> {
        self.home
            .as_deref()
            .ok_or_else(|| ClaudeUsageFailure::Internal("the home directory is unknown".to_owned()))
    }

    fn directory(&self) -> Result<PathBuf, ClaudeUsageFailure> {
        Ok(self.home()?.join(FEATURE_DIRECTORY))
    }

    /// Claude Code 的設定資料夾:`CLAUDE_CONFIG_DIR`,沒設定是 `~/.claude`。
    fn config_directory(&self) -> Result<PathBuf, ClaudeUsageFailure> {
        Ok(self
            .claude_config_dir
            .clone()
            .unwrap_or_else(|| self.home.clone().unwrap_or_default().join(".claude")))
    }

    fn settings_path(&self) -> Result<PathBuf, ClaudeUsageFailure> {
        self.home()?;
        Ok(self.config_directory()?.join("settings.json"))
    }

    /// 安裝紀錄裡的路徑:相對家目錄(app 的 `HostPath`,以 `/` 分隔);設定資料夾在家目錄外時是絕對路徑。
    fn journal_key(&self, settings: &Path) -> Result<String, ClaudeUsageFailure> {
        let text = |path: &Path| {
            path.components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        };
        match settings.strip_prefix(self.home()?) {
            Ok(relative) => Ok(text(relative)),
            Err(_) => Ok(settings.to_string_lossy().replace('\\', "/")),
        }
    }

    /// 這台主機要寫的包裝指令(Windows 用 PowerShell 版,家目錄以 `/` 分隔;同 app 的 `wrapperCommand(for:home:)`)。
    fn wrapper_command(&self) -> Result<String, ClaudeUsageFailure> {
        if !self.windows {
            return Ok(SHELL_COMMAND.to_owned());
        }
        let home = self.home()?.to_string_lossy().replace('\\', "/");
        Ok(format!(
            "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File '{home}/{POWERSHELL_SCRIPT}'"
        ))
    }

    fn read_journal(&self) -> Result<Option<Journal>, ClaudeUsageFailure> {
        let path = self.directory()?.join("journal.json");
        match FileState::read(&path) {
            FileState::File(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| ClaudeUsageFailure::WriteFailed("the install journal is unreadable".to_owned())),
            FileState::Unusable => Err(ClaudeUsageFailure::WriteFailed("the install journal is unreadable".to_owned())),
            FileState::Missing | FileState::MissingDirectory => Ok(None),
        }
    }

    fn inspect(&self) -> Result<Inspection, ClaudeUsageFailure> {
        Ok(Inspection {
            settings: FileState::read(&self.settings_path()?),
            journal: self.read_journal()?,
        })
    }

    fn state(&self) -> Result<ClaudeUsageState, ClaudeUsageFailure> {
        let inspection = self.inspect()?;
        Ok(ClaudeUsageState {
            installed: inspection.journal.is_some(),
            wrapper_active: is_wrapper(status_line_command(inspection.settings.contents()).as_deref()),
            claude_found: self.config_directory()?.is_dir(),
        })
    }

    pub async fn status(&self) -> Result<ClaudeUsageState, ClaudeUsageFailure> {
        let _serial = SERIAL.lock().await;
        self.state()
    }

    /// 安裝包裝。只在讀取後 `settings.json` 沒被改過時寫入。
    pub async fn install(&self) -> Result<ClaudeUsageState, ClaudeUsageFailure> {
        let _serial = SERIAL.lock().await;
        let settings_path = self.settings_path()?;
        let key = self.journal_key(&settings_path)?;
        let inspection = self.inspect()?;
        let mut settings = match &inspection.settings {
            FileState::MissingDirectory => return Err(ClaudeUsageFailure::ClaudeNotFound),
            FileState::Missing => Map::new(),
            FileState::File(bytes) => json_object(bytes).ok_or(ClaudeUsageFailure::SettingsNotJson)?,
            FileState::Unusable => {
                return Err(ClaudeUsageFailure::WriteFailed(
                    "settings.json is not a regular file or cannot be read".to_owned(),
                ));
            }
        };
        // 安裝前的內容:有安裝紀錄就用紀錄裡的(重複安裝不會把安裝後的內容當成原本的)。
        let recorded = inspection
            .journal
            .as_ref()
            .and_then(|journal| journal.entries.iter().find(|entry| entry.path == key));
        let original: Option<Vec<u8>> = match recorded {
            Some(entry) => entry.original.clone(),
            None => inspection.settings.contents().map(<[u8]>::to_vec),
        };
        // 包裝本身永遠不會被存成「原本的」:否則執行時會一直呼叫自己。
        let original_command = status_line_command(original.as_deref()).filter(|command| !is_wrapper(Some(command)));

        let mut status_line = settings.get("statusLine").and_then(Value::as_object).cloned().unwrap_or_default();
        status_line.insert("type".to_owned(), json!("command"));
        status_line.insert("command".to_owned(), json!(self.wrapper_command()?));
        settings.insert("statusLine".to_owned(), Value::Object(status_line));
        let installed = render_settings(&settings);

        let mut entries: Vec<JournalEntry> = inspection
            .journal
            .as_ref()
            .map(|journal| journal.entries.iter().filter(|entry| entry.path != key).map(clone_entry).collect())
            .unwrap_or_default();
        entries.push(JournalEntry {
            installed: installed.clone(),
            original,
            path: key,
        });
        let journal = Journal {
            created_directories: inspection.journal.and_then(|journal| journal.created_directories),
            entries,
            version: 1,
        };

        // 先寫自己的檔案，設定檔最後寫:中途失敗時使用者的設定檔還沒動。
        let directory = self.directory()?;
        if FileState::read(&settings_path) != inspection.settings {
            return Err(changed(&settings_path));
        }
        let encoded = serde_json::to_vec(&journal).map_err(|error| ClaudeUsageFailure::Internal(error.to_string()))?;
        write_atomically(&directory.join("journal.json"), &encoded, false)?;
        if self.windows {
            write_atomically(&directory.join("statusline.ps1"), POWERSHELL_WRAPPER.as_bytes(), false)?;
        } else {
            write_atomically(&directory.join("statusline.sh"), SHELL_WRAPPER.as_bytes(), true)?;
        }
        if let Some(command) = original_command {
            write_atomically(&directory.join("original-command"), command.as_bytes(), false)?;
        }
        write_unless_changed(&settings_path, &inspection.settings, &installed)?;
        self.state()
    }

    /// 還原原本的設定，移除 `~/.purrdr/claude-usage/`。沒有安裝紀錄時什麼都不做。
    pub async fn remove(&self) -> Result<ClaudeUsageState, ClaudeUsageFailure> {
        let _serial = SERIAL.lock().await;
        let Some(journal) = self.read_journal()? else {
            return self.state();
        };
        // 先算好每個檔案要變成什麼(含路徑檢查與 merge),全部算得出來才動手。
        let mut changes: Vec<(PathBuf, FileState, Option<Vec<u8>>)> = Vec::new();
        for entry in &journal.entries {
            let path = self.entry_path(&entry.path)?;
            let state = FileState::read(&path);
            let current = match &state {
                FileState::File(current) => current,
                // 安裝後使用者自己刪掉了:尊重使用者，不再建立。
                FileState::Missing | FileState::MissingDirectory => continue,
                FileState::Unusable => {
                    return Err(ClaudeUsageFailure::WriteFailed(
                        "settings.json is not a regular file or cannot be read".to_owned(),
                    ));
                }
            };
            let target = if *current == entry.installed {
                entry.original.clone()
            } else {
                Some(merge(current, entry.original.as_deref())?)
            };
            if target.as_deref() != Some(current.as_slice()) {
                changes.push((path, state.clone(), target));
            }
        }
        for (path, expected, target) in changes {
            match target {
                Some(contents) => write_unless_changed(&path, &expected, &contents)?,
                None => {
                    if FileState::read(&path) != expected {
                        return Err(changed(&path));
                    }
                    fs::remove_file(&path)?;
                }
            }
        }
        // 自己建的資料夾由深到淺，空了才移除(使用者後來放了東西就保留)。
        for directory in journal.created_directories.iter().flatten().rev() {
            if let Some(path) = self.relative_path(directory) {
                let _ = fs::remove_dir(path);
            }
        }
        match fs::remove_dir_all(self.directory()?) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        self.state()
    }

    /// 家目錄底下、只由一般路徑段組成的相對路徑(不接受 `..` 與絕對路徑)。
    fn relative_path(&self, text: &str) -> Option<PathBuf> {
        let path = Path::new(text);
        let safe = !text.is_empty() && path.components().all(|component| matches!(component, Component::Normal(_)));
        safe.then(|| self.home.as_ref().map(|home| home.join(path))).flatten()
    }

    /// 安裝紀錄裡的路徑換成要還原的檔案:只能是 Claude 的 `settings.json`(目前的,或家目錄底下的相對路徑)。
    fn entry_path(&self, text: &str) -> Result<PathBuf, ClaudeUsageFailure> {
        let settings = self.settings_path()?;
        if self.journal_key(&settings)? == text {
            return Ok(settings);
        }
        match self.relative_path(text) {
            Some(path) if path.file_name().is_some_and(|name| name == "settings.json") => Ok(path),
            _ => Err(ClaudeUsageFailure::WriteFailed(
                "the install journal names a file that is not Claude's settings.json".to_owned(),
            )),
        }
    }

    // MARK: - 協定

    /// 處理 `usage.claude`、`usage.session_context` 與 `claude_usage.*` 請求(其他訊息不會送到這裡)。
    pub async fn handle(&self, message: ClientMessage) -> ServerMessage {
        match message {
            ClientMessage::UsageClaude(request) => match self.plan_usage() {
                Ok(usage) => ServerMessage::UsageClaudeResult(UsageClaudeResult {
                    request_id: request.request_id,
                    installed: usage.installed,
                    captured_at: usage.captured_at,
                    periods: usage.periods,
                }),
                Err(failure) => failure_reply(request.request_id, failure),
            },
            ClientMessage::UsageSessionContext(request) => match self.session_context(request.session_ids.as_deref()) {
                Ok(sessions) => ServerMessage::UsageSessionContextResult(UsageSessionContextResult {
                    request_id: request.request_id,
                    sessions,
                }),
                Err(failure) => failure_reply(request.request_id, failure),
            },
            ClientMessage::ClaudeUsageStatus(request) => match self.status().await {
                Ok(state) => ServerMessage::ClaudeUsageStatusResult(ClaudeUsageStatusResult {
                    request_id: request.request_id,
                    installed: state.installed,
                    wrapper_active: state.wrapper_active,
                    claude_found: state.claude_found,
                }),
                Err(failure) => failure_reply(request.request_id, failure),
            },
            ClientMessage::ClaudeUsageInstall(request) => match self.install().await {
                Ok(state) => ServerMessage::ClaudeUsageInstallResult(ClaudeUsageInstallResult {
                    request_id: request.request_id,
                    installed: state.installed,
                    wrapper_active: state.wrapper_active,
                    claude_found: state.claude_found,
                }),
                Err(failure) => failure_reply(request.request_id, failure),
            },
            ClientMessage::ClaudeUsageRemove(request) => match self.remove().await {
                Ok(state) => ServerMessage::ClaudeUsageRemoveResult(ClaudeUsageRemoveResult {
                    request_id: request.request_id,
                    installed: state.installed,
                    wrapper_active: state.wrapper_active,
                    claude_found: state.claude_found,
                }),
                Err(failure) => failure_reply(request.request_id, failure),
            },
            _ => error(None, ErrorCode::InternalError, "not a Claude usage request"),
        }
    }

    // MARK: - 讀取用量

    /// 整台主機的方案用量(`usage.json`)。沒有檔案、檔案壞掉都是空的 periods。
    pub fn plan_usage(&self) -> Result<PlanUsage, ClaudeUsageFailure> {
        let directory = self.directory()?;
        let installed = directory.join("journal.json").is_file();
        let empty = PlanUsage {
            installed,
            captured_at: None,
            periods: Vec::new(),
        };
        let Some(file) = read_json_file(&directory.join("usage.json")) else {
            return Ok(empty);
        };
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |elapsed| elapsed.as_secs_f64());
        let limits = &file["status_line_input"]["rate_limits"];
        let mut periods = Vec::new();
        for (key, kind) in [("five_hour", PlanUsagePeriodKind::FiveHour), ("seven_day", PlanUsagePeriodKind::Week)] {
            let Some(used) = limits[key]["used_percentage"].as_f64().filter(|used| used.is_finite()) else {
                continue;
            };
            let resets = epoch_seconds(&limits[key]["resets_at"]);
            // 已過重置時間:那個視窗已經重置，檔案裡的比例過時了。
            if resets.is_some_and(|resets| resets <= now) {
                continue;
            }
            periods.push(PlanUsagePeriod {
                kind,
                used_percentage: used.clamp(0.0, 100.0),
                resets_at: resets.map(format_time),
            });
        }
        Ok(PlanUsage {
            installed,
            captured_at: epoch_seconds(&file["captured_at"]).map(format_time),
            periods,
        })
    }

    /// 每個 session 的 context(`sessions/<session_id>.json`)。`None`:資料夾裡全部(新到舊)。
    pub fn session_context(&self, session_ids: Option<&[String]>) -> Result<Vec<SessionContext>, ClaudeUsageFailure> {
        let sessions = self.directory()?.join("sessions");
        let Some(ids) = session_ids else {
            let mut found: Vec<(f64, SessionContext)> = fs::read_dir(&sessions)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let id = name.strip_suffix(".json").filter(|id| is_valid_session_id(id))?.to_owned();
                    read_session(&sessions, &id)
                })
                .collect();
            found.sort_by(|left, right| right.0.total_cmp(&left.0));
            found.truncate(MAX_SESSIONS);
            return Ok(found.into_iter().map(|(_, session)| session).collect());
        };
        if ids.len() > MAX_SESSIONS {
            return Err(ClaudeUsageFailure::InvalidRequest("too many session_ids"));
        }
        if !ids.iter().all(|id| is_valid_session_id(id)) {
            return Err(ClaudeUsageFailure::InvalidRequest(
                "session_ids must be 1-128 characters of A-Za-z0-9._- and not . or ..",
            ));
        }
        Ok(ids.iter().filter_map(|id| read_session(&sessions, id)).map(|(_, session)| session).collect())
    }
}

fn failure_reply(request_id: String, failure: ClaudeUsageFailure) -> ServerMessage {
    let request_id = Some(request_id);
    match failure {
        ClaudeUsageFailure::InvalidRequest(message) => error(request_id, ErrorCode::InvalidMessage, message),
        ClaudeUsageFailure::ClaudeNotFound => error(request_id, ErrorCode::ClaudeNotFound, "Claude Code settings directory was not found"),
        ClaudeUsageFailure::SettingsNotJson => error(request_id, ErrorCode::ClaudeSettingsNotJson, "Claude settings.json is not a valid JSON object"),
        ClaudeUsageFailure::WriteFailed(message) => error(request_id, ErrorCode::HostWriteFailed, &message),
        ClaudeUsageFailure::Internal(message) => error(request_id, ErrorCode::InternalError, &message),
    }
}

fn clone_entry(entry: &JournalEntry) -> JournalEntry {
    JournalEntry {
        installed: entry.installed.clone(),
        original: entry.original.clone(),
        path: entry.path.clone(),
    }
}

/// 使用者在安裝後又改過設定:只撤掉 `statusLine` 的修改(使用者已換掉 status line 就不動),其他內容保留。
fn merge(current: &[u8], original: Option<&[u8]>) -> Result<Vec<u8>, ClaudeUsageFailure> {
    let mut object = json_object(current).ok_or(ClaudeUsageFailure::SettingsNotJson)?;
    if !is_wrapper(status_line_command(Some(current)).as_deref()) {
        return Ok(current.to_vec());
    }
    match original
        .and_then(json_object)
        .and_then(|original| original.get("statusLine").filter(|value| value.is_object()).cloned())
    {
        Some(status_line) => object.insert("statusLine".to_owned(), status_line),
        None => object.remove("statusLine"),
    };
    Ok(render_settings(&object))
}

// MARK: - 用量檔

/// 與包裝腳本同一條規則:1–128 個 `A-Za-z0-9._-`,不是 `.` 或 `..`。
fn is_valid_session_id(id: &str) -> bool {
    (1..=128).contains(&id.len()) && id != "." && id != ".." && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// 只讀資料夾裡的一般檔案(不跟隨 symlink)並解析成 JSON;太大、讀不到、壞掉都是 `None`。
fn read_json_file(path: &Path) -> Option<Value> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn epoch_seconds(value: &Value) -> Option<f64> {
    value.as_f64().filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
}

fn format_time(seconds: f64) -> String {
    humantime::format_rfc3339_seconds(UNIX_EPOCH + Duration::from_secs(seconds as u64)).to_string()
}

/// 一個 session 檔;回傳 captured_at(排序用)與內容。
fn read_session(directory: &Path, id: &str) -> Option<(f64, SessionContext)> {
    let file = read_json_file(&directory.join(format!("{id}.json")))?;
    let captured = epoch_seconds(&file["captured_at"])?;
    let context = &file["status_line_input"]["context_window"];
    let size = context["context_window_size"].as_i64().or_else(|| {
        context["context_window_size"]
            .as_f64()
            .filter(|size| size.fract() == 0.0 && size.abs() < 9e15)
            .map(|size| size as i64)
    });
    Some((
        captured,
        SessionContext {
            session_id: id.to_owned(),
            captured_at: format_time(captured),
            used_percentage: context["used_percentage"]
                .as_f64()
                .filter(|used| used.is_finite())
                .map(|used| used.clamp(0.0, 100.0)),
            context_window_size: size.filter(|size| *size >= 0),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_hosts_use_the_powershell_command_form_of_the_app() {
        let usage = ClaudeUsage {
            home: Some(PathBuf::from("C:\\Users\\ray")),
            claude_config_dir: None,
            windows: true,
        };

        let command = usage.wrapper_command().unwrap();

        assert_eq!(
            command,
            "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File 'C:/Users/ray/.purrdr/claude-usage/statusline.ps1'"
        );
        assert!(is_wrapper(Some(&command)));
    }

    #[test]
    fn only_purrdrs_own_commands_are_wrappers() {
        assert!(is_wrapper(Some(SHELL_COMMAND)));
        assert!(!is_wrapper(None));
        assert!(!is_wrapper(Some("echo hi")));
        assert!(!is_wrapper(Some("powershell.exe -File 'C:/x/other.ps1'")));
        assert!(!is_wrapper(Some("~/.purrdr/claude-usage/statusline.sh --extra")));
    }

    #[test]
    fn a_changed_file_is_not_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, "a").unwrap();
        let read = FileState::read(&path);
        fs::write(&path, "changed by someone else").unwrap();

        assert!(matches!(write_unless_changed(&path, &read, b"mine"), Err(ClaudeUsageFailure::WriteFailed(_))));
        assert_eq!(fs::read_to_string(&path).unwrap(), "changed by someone else");
        assert!(write_unless_changed(&path, &FileState::read(&path), b"mine").is_ok());
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine");
    }
}
