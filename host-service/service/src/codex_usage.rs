//! Codex 的方案用量(raychiutw/purrdr.host#62):以本機使用者身分執行一次 `codex app-server`,
//! 讀 `account/rateLimits/read`(與 app 經 SSH 的 `CodexPlanUsageHost` 同一套交談)。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{ChildStdout, Command};

use crate::protocol::{PlanUsagePeriod, PlanUsagePeriodKind};

/// 整段交談的期限(app-server 啟動要載入設定)。
const TIMEOUT: Duration = Duration::from_secs(20);
/// 沒登入 ChatGPT(或用 API key 登入)時錯誤訊息含這段(openai/codex `account_processor.rs`;同 app 的判斷)。
const NOT_SIGNED_IN_FRAGMENT: &str = "authentication required";
/// 週期長度不到一天算 5 小時的週期。
const DAY_MINUTES: f64 = 24.0 * 60.0;

/// Windows:不開 console 視窗(背景執行檔沒有 console)。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 要執行的 codex。
#[derive(Debug, Clone, Default)]
pub struct CodexCli {
    /// `None`:每次執行時才找(codex 可能在主機服務啟動後才安裝)。
    program: Option<PathBuf>,
    /// 執行檔後面固定的參數(測試的假 codex)。
    program_arguments: Vec<String>,
}

impl CodexCli {
    /// 每次執行時才找 codex。
    pub fn search() -> Self {
        CodexCli::default()
    }

    /// 固定的執行檔與參數(測試)。
    pub fn at_path(program: PathBuf, program_arguments: Vec<String>) -> Self {
        CodexCli {
            program: Some(program),
            program_arguments,
        }
    }
}

/// 讀不到 Codex 用量的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CodexUsageFailure {
    NotFound,
    NotSignedIn,
    /// 帶原因。
    Failed(String),
}

/// 讀到的用量：讀取時間(RFC 3339)與週期。
pub(crate) struct CodexUsage {
    pub captured_at: String,
    pub periods: Vec<PlanUsagePeriod>,
}

impl CodexCli {
    /// 要執行的檔案;找不到時 `None`。
    fn program(&self) -> Option<PathBuf> {
        self.program.clone().or_else(locate)
    }
}

/// 依序找 PATH 與常見的安裝位置(自動啟動時 PATH 只有系統目錄):Homebrew、`/usr/local/bin`、
/// `~/.local/bin`、npm 的全域位置;Windows 是 PATH 與 `%APPDATA%\npm` 的 `codex.exe`／`codex.cmd`。
fn locate() -> Option<PathBuf> {
    let variable = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    let mut directories: Vec<PathBuf> = variable("PATH").map(|path| std::env::split_paths(&path).collect()).unwrap_or_default();
    let (names, extra): (&[&str], Vec<PathBuf>) = if cfg!(windows) {
        (
            &["codex.exe", "codex.cmd"],
            variable("APPDATA").map(|app_data| vec![Path::new(&app_data).join("npm")]).unwrap_or_default(),
        )
    } else {
        let mut extra = vec![PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")];
        if let Some(home) = variable("HOME") {
            let home = Path::new(&home);
            extra.extend([home.join(".local/bin"), home.join(".npm-global/bin"), home.join("bin")]);
        }
        (&["codex"], extra)
    };
    directories.extend(extra);
    directories
        .iter()
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|candidate| candidate.is_file())
}

/// 執行一次 `codex app-server`:`initialize` → `initialized` → `account/rateLimits/read`,讀到回應就結束子程序。
pub(crate) async fn read(cli: &CodexCli) -> Result<CodexUsage, CodexUsageFailure> {
    let program = cli.program().ok_or(CodexUsageFailure::NotFound)?;
    let mut command = Command::new(&program);
    command
        .args(&cli.program_arguments)
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let mut child = command.spawn().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => CodexUsageFailure::NotFound,
        _ => CodexUsageFailure::Failed(format!("cannot start codex: {error}")),
    })?;
    let captured_at = humantime::format_rfc3339_seconds(SystemTime::now()).to_string();
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Err(CodexUsageFailure::Failed("cannot talk to codex app-server".to_owned()));
    };
    let exchange = async {
        let mut lines = BufReader::new(stdout).lines();
        let initialize = json!({"method": "initialize", "id": 0, "params": {"clientInfo": {"name": "purrdr", "title": "Purrdr", "version": "1.0"}}});
        send(&mut stdin, &[initialize]).await?;
        result_for(&mut lines, 0).await?;
        send(
            &mut stdin,
            &[json!({"method": "initialized"}), json!({"method": "account/rateLimits/read", "id": 1})],
        )
        .await?;
        result_for(&mut lines, 1).await
    };
    let outcome = tokio::time::timeout(TIMEOUT, exchange).await;
    let _ = child.start_kill();
    let result = match outcome {
        Ok(Ok(result)) => result,
        Ok(Err(Ended)) => {
            let status = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            return Err(match status {
                Ok(Ok(status)) if status.code() == Some(127) => CodexUsageFailure::NotFound,
                _ => CodexUsageFailure::Failed("codex app-server ended without an answer".to_owned()),
            });
        }
        Ok(Err(AppServer(failure))) => return Err(failure),
        Err(_) => return Err(CodexUsageFailure::Failed("codex app-server did not answer in time".to_owned())),
    };
    Ok(CodexUsage {
        captured_at,
        periods: periods(&result).ok_or_else(|| CodexUsageFailure::Failed("cannot read the rate limits codex returned".to_owned()))?,
    })
}

/// 交談沒有拿到結果的原因。
enum ExchangeFailure {
    /// 子程序結束或關閉了輸出。
    Ended,
    /// app-server 回報錯誤，或寫不進去。
    AppServer(CodexUsageFailure),
}
use ExchangeFailure::{AppServer, Ended};

/// 一行一則 JSON-RPC(不帶 jsonrpc 欄位,同 codex app-server 的 stdio)。
async fn send(stdin: &mut tokio::process::ChildStdin, messages: &[Value]) -> Result<(), ExchangeFailure> {
    let text: String = messages.iter().map(|message| format!("{message}\n")).collect();
    stdin.write_all(text.as_bytes()).await.map_err(|_| Ended)?;
    stdin.flush().await.map_err(|_| Ended)
}

/// 讀到指定 `id` 的回應為止(通知與其他回應略過)。
async fn result_for(lines: &mut Lines<BufReader<ChildStdout>>, id: i64) -> Result<Value, ExchangeFailure> {
    while let Some(line) = lines.next_line().await.map_err(|_| Ended)? {
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        if message["id"].as_i64() != Some(id) {
            continue;
        }
        if let Some(error) = message.get("error") {
            let text = error["message"].as_str().unwrap_or_default();
            return Err(AppServer(if text.contains(NOT_SIGNED_IN_FRAGMENT) {
                CodexUsageFailure::NotSignedIn
            } else {
                CodexUsageFailure::Failed(text.to_owned())
            }));
        }
        return Ok(message["result"].clone());
    }
    Err(Ended)
}

/// 多 bucket 時用 Codex 本身的(`codex`);primary／secondary 依週期長度對應 5 小時與每週(同 app 的 `planUsage`)。
fn periods(result: &Value) -> Option<Vec<PlanUsagePeriod>> {
    let snapshot = result["rateLimitsByLimitId"].get("codex").or_else(|| result.get("rateLimits"))?.as_object()?;
    let mut periods: Vec<PlanUsagePeriod> = Vec::new();
    for (key, slot) in [("primary", PlanUsagePeriodKind::FiveHour), ("secondary", PlanUsagePeriodKind::Week)] {
        let Some(used) = snapshot.get(key).and_then(|period| period["usedPercent"].as_f64()) else {
            continue;
        };
        let period = &snapshot[key];
        let kind = match period["windowDurationMins"].as_f64() {
            Some(minutes) if minutes < DAY_MINUTES => PlanUsagePeriodKind::FiveHour,
            Some(_) => PlanUsagePeriodKind::Week,
            None => slot,
        };
        if periods.iter().any(|existing| existing.kind == kind) {
            continue;
        }
        let resets_at = period["resetsAt"]
            .as_u64()
            .map(|seconds| humantime::format_rfc3339_seconds(UNIX_EPOCH + Duration::from_secs(seconds)).to_string());
        periods.push(PlanUsagePeriod {
            kind,
            used_percentage: used.clamp(0.0, 100.0).round() as i64,
            resets_at,
        });
    }
    periods.sort_by_key(|period| period.kind != PlanUsagePeriodKind::FiveHour);
    Some(periods)
}
