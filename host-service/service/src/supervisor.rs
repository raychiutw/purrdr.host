//! Windows 背景執行檔的看門狗(raychiutw/purrdr.host#112)。
//!
//! Windows 登入時只由 HKCU Run 機碼啟動一次,程序消失後沒有人把它帶回來。
//! `purrdr-host-service-background.exe` 因此先當看門狗:以自己加 `--supervised-child` 啟動真正的主機服務,
//! 子程序消失就在退避間隔後重啟,連續快速失敗到上限就放棄(避免無限快速重啟迴圈)。
//!
//! 使用者主動停止(安裝、更新、移除)時,安裝流程先寫下 `stop-requested` 標記再 `taskkill`,
//! 看門狗重啟前檢查標記,有就結束。看門狗與子程序同映像名,`taskkill /IM` 會一起結束兩者。
//! 子程序在 Windows 上放進設了 `KILL_ON_JOB_CLOSE` 的 Job Object:看門狗不論怎麼結束(含被強制結束),
//! 子程序都跟著結束,不會變成沒人監看的孤兒(之後新的看門狗再啟動第二份服務會 bind 失敗)。

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::shutdown::{ProcessTable, same_process};

/// 子程序以這個旗標(放在第一個參數)啟動時直接當主機服務執行,不再當看門狗。
pub const CHILD_FLAG: &str = "--supervised-child";
/// 參數錯誤的結束碼(`run_service`):重啟也不會好,不重啟。
const USAGE_ERROR_CODE: i32 = 2;
/// 等待退避時多久檢查一次停止標記。
const STOP_POLL: Duration = Duration::from_secs(1);

/// 重啟策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// 子程序至少活這麼久才算健康,失敗計數歸零。
    pub healthy_after: Duration,
    /// 第一次重啟前的等待;之後每連續失敗一次加倍。
    pub base_delay: Duration,
    pub max_delay: Duration,
    /// 連續快速失敗這麼多次就放棄。
    pub max_consecutive_failures: u32,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            healthy_after: Duration::from_secs(60),
            base_delay: Duration::from_secs(5),
            max_delay: Duration::from_secs(300),
            max_consecutive_failures: 10,
        }
    }
}

/// 子程序結束的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildExit {
    pub code: Option<i32>,
    pub lived: Duration,
}

/// 看門狗與外界的接點(啟動子程序、等待、log),測試換成假的。
pub trait Supervised {
    /// 啟動子程序並等它結束。
    fn run_child(&mut self) -> io::Result<ChildExit>;
    fn stop_requested(&mut self) -> bool;
    /// 等待退避間隔(實作應該在等待中也留意停止標記)。
    fn sleep(&mut self, duration: Duration);
    fn log(&mut self, message: &str);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 安裝流程要求停止,或服務自己正常關閉(結束碼 0)。
    StopRequested,
    /// 子程序因參數錯誤結束。
    UsageError,
    /// 連續快速失敗達上限。
    GaveUp { failures: u32 },
}

pub fn supervise(environment: &mut impl Supervised, policy: &Policy) -> Outcome {
    let mut failures = 0;
    let mut delay = policy.base_delay;
    loop {
        let (code, lived) = match environment.run_child() {
            Ok(exit) => (exit.code, exit.lived),
            Err(error) => {
                environment.log(&format!("watchdog: cannot start the service: {error}"));
                (None, Duration::ZERO)
            }
        };
        if code == Some(USAGE_ERROR_CODE) {
            environment.log("watchdog: the service rejected its arguments, not restarting");
            return Outcome::UsageError;
        }
        if code == Some(0) {
            // 服務只在收到結束訊號(登出、關機、console close、Ctrl+C)後走正常關閉才會回 0
            // (聽連線的迴圈意外結束回 1、panic 回 101、被強制結束回非 0):這是有意的停止,不該拉回來。
            environment.log("watchdog: the service shut down normally, not restarting");
            return Outcome::StopRequested;
        }
        if environment.stop_requested() {
            return Outcome::StopRequested;
        }
        if lived >= policy.healthy_after {
            failures = 0;
            delay = policy.base_delay;
        } else {
            failures += 1;
            if failures >= policy.max_consecutive_failures {
                environment.log(&format!(
                    "watchdog: the service failed {failures} times in a row, giving up until the next login"
                ));
                return Outcome::GaveUp { failures };
            }
            if failures > 1 {
                delay = (delay * 2).min(policy.max_delay);
            }
        }
        environment.log(&format!(
            "watchdog: the service exited ({}) after {}s, restarting in {}s",
            code.map_or_else(|| "no exit code".to_owned(), |code| format!("code {code}")),
            lived.as_secs(),
            delay.as_secs()
        ));
        environment.sleep(delay);
        if environment.stop_requested() {
            return Outcome::StopRequested;
        }
    }
}

pub fn stop_marker(state_directory: &Path) -> PathBuf {
    state_directory.join("stop-requested")
}

/// 安裝流程停止服務之前呼叫,讓看門狗不要把它帶回來。標記寫不下去時回錯誤:
/// 呼叫端不該在這種情況下繼續 `taskkill`(看門狗會把舊服務拉回來)。
pub fn request_stop(state_directory: &Path) -> io::Result<()> {
    std::fs::create_dir_all(state_directory)?;
    std::fs::write(stop_marker(state_directory), "")
}

/// 輪詢到 `is_running` 回 false 為止(回 true);最多再等 `attempts` 次、每次 `interval`,等不到回 false。
/// 檢查與等待都由呼叫端注入,測試不必真的等。
pub fn wait_until_gone(mut is_running: impl FnMut() -> bool, mut sleep: impl FnMut(Duration), interval: Duration, attempts: u32) -> bool {
    for _ in 0..attempts {
        if !is_running() {
            return true;
        }
        sleep(interval);
    }
    !is_running()
}

/// 舊的程序都已結束、要啟動新的之前呼叫(新看門狗啟動時也會自己清)。
pub fn clear_stop(state_directory: &Path) {
    let _ = std::fs::remove_file(stop_marker(state_directory));
}

/// 子程序與看門狗同生共死(Windows Job Object);其他平台不需要(沒有看門狗)。
#[cfg(windows)]
#[allow(unsafe_code)] // 只有這個模組:Job Object 的 Win32 呼叫。
mod job {
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };

    /// 設了 `KILL_ON_JOB_CLOSE` 的 Job Object:最後一個 handle 關閉(看門狗結束,不論怎麼結束)時,裡面的程序一併結束。
    pub struct ChildJob(Option<HANDLE>);

    impl ChildJob {
        pub fn new() -> Self {
            Self(create().ok())
        }

        pub fn adopt(&self, child: &Child) -> io::Result<()> {
            let job = self.0.ok_or_else(|| io::Error::other("the job object could not be created"))?;
            // SAFETY: `job` 是有效的 job handle;`child` 在呼叫期間持有有效的程序 handle。
            if unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Drop for ChildJob {
        fn drop(&mut self) {
            if let Some(job) = self.0.take() {
                // SAFETY: 自己建立、只關閉一次;關閉即觸發 KILL_ON_JOB_CLOSE。
                unsafe { CloseHandle(job) };
            }
        }
    }

    fn create() -> io::Result<HANDLE> {
        // SAFETY: 建立匿名 job;結構以零初始化後只設 LimitFlags,大小與資訊類別對應;失敗時關掉 handle。
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if configured == 0 {
                let error = io::Error::last_os_error();
                CloseHandle(job);
                return Err(error);
            }
            Ok(job)
        }
    }
}

#[cfg(not(windows))]
mod job {
    use std::io;
    use std::process::Child;

    pub struct ChildJob;

    impl ChildJob {
        pub fn new() -> Self {
            Self
        }

        pub fn adopt(&self, _child: &Child) -> io::Result<()> {
            Ok(())
        }
    }
}

/// 真的啟動子程序的環境:子程序是自己的執行檔加 [`CHILD_FLAG`],參數原樣轉交。
struct ChildProcess {
    executable: PathBuf,
    arguments: Vec<String>,
    state_directory: PathBuf,
    log: crate::service_log::ServiceLog,
    job: job::ChildJob,
}

impl ChildProcess {
    fn new(executable: PathBuf, arguments: Vec<String>, state_directory: PathBuf, log: crate::service_log::ServiceLog) -> Self {
        ChildProcess {
            executable,
            arguments,
            state_directory,
            log,
            job: job::ChildJob::new(),
        }
    }
}

impl Supervised for ChildProcess {
    fn run_child(&mut self) -> io::Result<ChildExit> {
        let started = Instant::now();
        let mut child = std::process::Command::new(&self.executable)
            .arg(CHILD_FLAG)
            .args(&self.arguments)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        if let Err(error) = self.job.adopt(&child) {
            // 綁不上時服務照跑(只是失去「看門狗結束就一起結束」的保護),記下來不中斷。
            self.log.record(&format!("watchdog: cannot tie the service to the watchdog: {error}"));
        }
        let status = child.wait()?;
        Ok(ChildExit {
            code: status.code(),
            lived: started.elapsed(),
        })
    }

    fn stop_requested(&mut self) -> bool {
        stop_marker(&self.state_directory).exists()
    }

    fn sleep(&mut self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while !self.stop_requested() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return;
            }
            std::thread::sleep(remaining.min(STOP_POLL));
        }
    }

    fn log(&mut self, message: &str) {
        self.log.record(message);
    }
}

/// 看門狗的狀態檔(`watchdog.json`):`status` 據此顯示看門狗是否真的在跑、是否已放棄。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchdogRecord {
    pub pid: u32,
    pub state: WatchdogState,
    pub started_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchdogState {
    Running,
    /// 連續快速失敗達上限,看門狗已結束,不會再重啟。
    GaveUp,
}

const WATCHDOG_FILE: &str = "watchdog.json";

pub fn write_watchdog(state_directory: &Path, record: &WatchdogRecord) -> io::Result<()> {
    std::fs::create_dir_all(state_directory)?;
    std::fs::write(state_directory.join(WATCHDOG_FILE), serde_json::to_string(record).expect("可序列化"))
}

pub fn read_watchdog(state_directory: &Path) -> Option<WatchdogRecord> {
    serde_json::from_str(&std::fs::read_to_string(state_directory.join(WATCHDOG_FILE)).ok()?).ok()
}

pub fn remove_watchdog(state_directory: &Path) {
    let _ = std::fs::remove_file(state_directory.join(WATCHDOG_FILE));
}

/// `status` 的「auto restart」說明,依看門狗實際狀態而不是只看有沒有登記。
pub fn watchdog_note(record: Option<&WatchdogRecord>, processes: &dyn ProcessTable) -> String {
    match record {
        Some(record) if record.state == WatchdogState::GaveUp => {
            "watchdog gave up after repeated failures (restart the service, or log in again, to re-arm it)".to_owned()
        }
        Some(record) if processes.is_alive(record.pid) && same_process(record.started_at_ms, processes.started_at_ms(record.pid)) => {
            "watchdog running (restarts the service if it disappears)".to_owned()
        }
        _ => "watchdog not running (configured at login; the service is not being watched)".to_owned(),
    }
}

/// 背景執行檔的進入點用:找得到資料夾與自己的執行檔時當看門狗並回傳結束碼;
/// 否則把原因記下來(`supervision disabled: <原因>`)並回 None,由呼叫端直接執行服務(沒有監看)。
pub fn run_watchdog(arguments: &[String]) -> Option<ExitCode> {
    let setup = resolve_state_directory(arguments, || {
        crate::state_directory::resolve_state_directory(
            crate::herdr_socket::Platform::current(),
            &crate::herdr_socket::SocketEnvironment::from_process(),
        )
    })
    .and_then(|directory| {
        std::env::current_exe()
            .map(|executable| (directory, executable))
            .map_err(|error| format!("cannot locate the service executable: {error}"))
    });
    let (state_directory, executable) = match setup {
        Ok(setup) => setup,
        Err(reason) => {
            report_supervision_disabled(&reason);
            return None;
        }
    };
    clear_stop(&state_directory);
    let log = crate::service_log::ServiceLog::under(&state_directory);
    let record = |state| WatchdogRecord {
        pid: std::process::id(),
        state,
        started_at_ms: crate::running_service::unix_millis(std::time::SystemTime::now()),
    };
    if let Err(error) = write_watchdog(&state_directory, &record(WatchdogState::Running)) {
        log.record(&format!("watchdog: cannot write watchdog.json: {error}"));
    }
    let mut child = ChildProcess::new(executable, arguments.to_vec(), state_directory.clone(), log);
    let outcome = supervise(&mut child, &Policy::default());
    match outcome {
        Outcome::GaveUp { .. } => {
            let _ = write_watchdog(&state_directory, &record(WatchdogState::GaveUp));
        }
        Outcome::StopRequested | Outcome::UsageError => remove_watchdog(&state_directory),
    }
    Some(match outcome {
        Outcome::StopRequested => ExitCode::SUCCESS,
        Outcome::UsageError => ExitCode::from(2),
        Outcome::GaveUp { .. } => ExitCode::FAILURE,
    })
}

/// `--state-dir <資料夾>`,沒有時用 `default`(預設的家目錄位置);兩者都沒有時回原因。
pub fn resolve_state_directory(arguments: &[String], default: impl FnOnce() -> Option<PathBuf>) -> Result<PathBuf, String> {
    match arguments.iter().position(|argument| argument == "--state-dir") {
        Some(index) => arguments.get(index + 1).map(PathBuf::from).ok_or_else(|| "--state-dir has no value".to_owned()),
        None => default().ok_or_else(|| "cannot determine the home directory and no --state-dir was given".to_owned()),
    }
}

/// 沒有資料夾可放 log 時的退路:stderr(可能沒人看)加系統暫存資料夾裡的 log。
fn report_supervision_disabled(reason: &str) {
    let message = format!("supervision disabled: {reason}");
    eprintln!("{message}");
    let line = format!("{} {message}", humantime::format_rfc3339_seconds(std::time::SystemTime::now()));
    let log = crate::service_log::RotatingLog::new(
        std::env::temp_dir().join("purrdr-host-service").join("logs").join("host-service.log"),
        crate::service_log::SERVICE_LOG_MAXIMUM_BYTES,
    );
    let _ = log.append_line(&line);
}
