//! 異常結束要留下證據(raychiutw/purrdr.host#109):正常結束寫一筆關閉原因並清掉 `running.json`;
//! 下次啟動時 `running.json` 還在、記載的程序卻已不存在,就記一筆「上次執行未正常關閉」。
//! 被強制結束(SIGKILL、`taskkill /F`)寫不出關閉紀錄,靠下次啟動的偵測補上。

use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use crate::running_service::{self, RunningService};
use crate::service_log::ServiceLog;

/// 查某個 pid 的程序還在不在。不執行外部指令;測試以假的取代,不必真的結束程序。
pub trait ProcessTable {
    fn is_alive(&self, pid: u32) -> bool;
}

/// 作業系統真正的程序表。
pub struct SystemProcesses;

impl ProcessTable for SystemProcesses {
    fn is_alive(&self, pid: u32) -> bool {
        system_is_alive(pid)
    }
}

/// signal 0 只檢查程序存在與否,不送訊號。
#[cfg(unix)]
#[allow(unsafe_code)] // 只有這個函式:呼叫 kill(pid, 0)。
fn system_is_alive(pid: u32) -> bool {
    // pid 0 與超出 pid_t 的值(kill 會當成程序群組或負數)一律視為不存在。
    let Ok(pid) = i32::try_from(pid) else { return false };
    if pid == 0 {
        return false;
    }
    // SAFETY: signal 0 不送出任何訊號,沒有記憶體相關的前提。
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    // EPERM:程序存在,只是不屬於自己。
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
#[allow(unsafe_code)] // 只有這個函式:查詢程序的退出碼。
fn system_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    const STILL_ACTIVE: u32 = 259;
    if pid == 0 {
        return false;
    }
    // SAFETY: 只查詢退出碼,並關掉自己開的 handle。
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let queried = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        queried && code == STILL_ACTIVE
    }
}

#[cfg(not(any(unix, windows)))]
fn system_is_alive(_pid: u32) -> bool {
    true
}

/// `running.json` 記載的程序已不存在(且不是自己)時回傳它的內容。
pub fn vanished_previous_run(state_directory: &Path, own_pid: u32, processes: &dyn ProcessTable) -> Option<RunningService> {
    let previous = running_service::read(state_directory)?;
    (previous.pid != own_pid && !processes.is_alive(previous.pid)).then_some(previous)
}

/// 上次執行的描述,log 與 `status` 共用:`pid 1234, version 0.17.0, started 2026-11-28T10:00:00Z`。
pub fn describe_run(run: &RunningService) -> String {
    let started = humantime::format_rfc3339_seconds(UNIX_EPOCH + Duration::from_millis(run.started_at_ms));
    format!("pid {}, version {}, started {started}", run.pid, run.version)
}

/// 啟動時呼叫:上次的程序消失了就記一筆,並清掉 `running.json`(只記一次,不管這次啟不啟得起來)。
pub fn report_unclean_previous_run(state_directory: &Path, log: &ServiceLog, own_pid: u32, processes: &dyn ProcessTable) {
    if let Some(previous) = vanished_previous_run(state_directory, own_pid, processes) {
        log.record(&format!(
            "previous run did not shut down cleanly: process is gone ({})",
            describe_run(&previous)
        ));
        running_service::remove(state_directory);
    }
}

/// 正常結束:記下原因,並清掉自己的 `running.json`(別人的不動)。
pub fn record_shutdown(state_directory: &Path, log: &ServiceLog, reason: &str) {
    log.record(&format!("stopping: {reason}"));
    if running_service::read(state_directory).is_some_and(|running| running.pid == std::process::id()) {
        running_service::remove(state_directory);
    }
}

/// panic 時先記一筆再交給原本的 hook(程序照常結束或繼續,由原本的行為決定)。
pub fn install_panic_hook(log: ServiceLog) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log.record(&format!("panic: {info}"));
        previous(info);
    }));
}

/// 等到收到結束訊號,回傳原因。
#[cfg(unix)]
pub async fn wait_for_termination() -> &'static str {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut interrupt), Ok(mut terminate), Ok(mut hangup)) =
        (signal(SignalKind::interrupt()), signal(SignalKind::terminate()), signal(SignalKind::hangup()))
    else {
        return std::future::pending().await;
    };
    tokio::select! {
        _ = interrupt.recv() => "ctrl-c (SIGINT)",
        _ = terminate.recv() => "terminate (SIGTERM)",
        _ = hangup.recv() => "hangup (SIGHUP)",
    }
}

#[cfg(windows)]
pub async fn wait_for_termination() -> &'static str {
    use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_logoff, ctrl_shutdown};
    let (Ok(mut interrupt), Ok(mut break_key), Ok(mut close), Ok(mut logoff), Ok(mut shutdown)) =
        (ctrl_c(), ctrl_break(), ctrl_close(), ctrl_logoff(), ctrl_shutdown())
    else {
        return std::future::pending().await;
    };
    tokio::select! {
        _ = interrupt.recv() => "ctrl-c",
        _ = break_key.recv() => "ctrl-break",
        _ = close.recv() => "console close",
        _ = logoff.recv() => "user logoff",
        _ = shutdown.recv() => "system shutdown",
    }
}

#[cfg(not(any(unix, windows)))]
pub async fn wait_for_termination() -> &'static str {
    std::future::pending().await
}
