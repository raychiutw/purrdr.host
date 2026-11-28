//! Windows 背景執行檔的看門狗(raychiutw/purrdr.host#112)。
//!
//! Windows 登入時只由 HKCU Run 機碼啟動一次,程序消失後沒有人把它帶回來。
//! `purrdr-host-service-background.exe` 因此先當看門狗:以自己加 `--supervised-child` 啟動真正的主機服務,
//! 子程序消失就在退避間隔後重啟,連續快速失敗到上限就放棄(避免無限快速重啟迴圈)。
//!
//! 使用者主動停止(安裝、更新、移除)時,安裝流程先寫下 `stop-requested` 標記再 `taskkill`,
//! 看門狗重啟前檢查標記,有就結束。看門狗與子程序同映像名,`taskkill /IM` 會一起結束兩者。

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

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
    /// 安裝流程要求停止。
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

/// 安裝流程停止服務之前呼叫,讓看門狗不要把它帶回來。
pub fn request_stop(state_directory: &Path) {
    let _ = std::fs::create_dir_all(state_directory);
    let _ = std::fs::write(stop_marker(state_directory), "");
}

/// 舊的程序都已結束、要啟動新的之前呼叫(新看門狗啟動時也會自己清)。
pub fn clear_stop(state_directory: &Path) {
    let _ = std::fs::remove_file(stop_marker(state_directory));
}

/// 真的啟動子程序的環境:子程序是自己的執行檔加 [`CHILD_FLAG`],參數原樣轉交。
struct ChildProcess {
    executable: PathBuf,
    arguments: Vec<String>,
    state_directory: PathBuf,
    log: crate::service_log::ServiceLog,
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

/// 背景執行檔的進入點用:找得到資料夾與自己的執行檔時當看門狗並回傳結束碼;否則回 None,由呼叫端直接執行服務。
pub fn run_watchdog(arguments: &[String]) -> Option<ExitCode> {
    let state_directory = state_directory_from(arguments)?;
    let executable = std::env::current_exe().ok()?;
    clear_stop(&state_directory);
    let log = crate::service_log::ServiceLog::under(&state_directory);
    let mut child = ChildProcess {
        executable,
        arguments: arguments.to_vec(),
        state_directory,
        log,
    };
    Some(match supervise(&mut child, &Policy::default()) {
        Outcome::StopRequested => ExitCode::SUCCESS,
        Outcome::UsageError => ExitCode::from(2),
        Outcome::GaveUp { .. } => ExitCode::FAILURE,
    })
}

/// `--state-dir <資料夾>`,沒有時用預設的家目錄位置。
fn state_directory_from(arguments: &[String]) -> Option<PathBuf> {
    match arguments.iter().position(|argument| argument == "--state-dir") {
        Some(index) => arguments.get(index + 1).map(PathBuf::from),
        None => crate::state_directory::resolve_state_directory(
            crate::herdr_socket::Platform::current(),
            &crate::herdr_socket::SocketEnvironment::from_process(),
        ),
    }
}
