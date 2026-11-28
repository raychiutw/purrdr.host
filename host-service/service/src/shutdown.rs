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

    /// 程序的啟動時間(Unix 毫秒);取不到就回 `None`(此時只能靠 `is_alive` 判斷)。
    /// 斷電、重開機或當機後 pid 常被別的程序重用,用它分辨「同一個 pid 的另一個程序」。
    fn started_at_ms(&self, _pid: u32) -> Option<u64> {
        None
    }
}

/// 作業系統真正的程序表。
pub struct SystemProcesses;

impl ProcessTable for SystemProcesses {
    fn is_alive(&self, pid: u32) -> bool {
        system_is_alive(pid)
    }

    fn started_at_ms(&self, pid: u32) -> Option<u64> {
        system_started_at_ms(pid)
    }
}

/// 程序啟動時間與 running.json 的 `started_at_ms` 之間容許的誤差:running.json 是啟動後才寫的,
/// 所以真正的同一個程序一定早於它;而 Linux 由開機時間與 clock ticks 推算只精確到約 1 秒。
const START_TIME_TOLERANCE_MS: u64 = 2_000;

/// 還活著的程序是不是 running.json 記載的那一個:啟動時間晚於記載的時間(超過誤差)就是 pid 被重用。
/// 取不到啟動時間時視為同一個(維持只看 pid 的行為)。
pub fn same_process(recorded_started_at_ms: u64, process_started_at_ms: Option<u64>) -> bool {
    process_started_at_ms.is_none_or(|started| started <= recorded_started_at_ms.saturating_add(START_TIME_TOLERANCE_MS))
}

/// `/proc/<pid>/stat` 的內容換算成啟動時間(Unix 毫秒):第 22 欄 starttime 是開機後的 clock ticks。
/// 第 2 欄 comm 可能含空白與括號,所以以最後一個 `)` 為界。
pub fn linux_start_time_ms(stat: &str, boot_time_seconds: u64, ticks_per_second: u64) -> Option<u64> {
    if ticks_per_second == 0 {
        return None;
    }
    let after_comm = &stat[stat.rfind(')')? + 1..];
    // `)` 之後從第 3 欄 state 開始,starttime(第 22 欄)是第 20 個。
    let ticks: u64 = after_comm.split_whitespace().nth(19)?.parse().ok()?;
    boot_time_seconds.checked_mul(1_000)?.checked_add(ticks.checked_mul(1_000)? / ticks_per_second)
}

/// Windows FILETIME(1601 年起算的 100 奈秒)換成 Unix 毫秒;0 或早於 1970 回 `None`。
pub fn filetime_to_unix_ms(filetime: u64) -> Option<u64> {
    const UNIX_EPOCH_AS_FILETIME: u64 = 116_444_736_000_000_000;
    filetime.checked_sub(UNIX_EPOCH_AS_FILETIME).map(|ticks| ticks / 10_000)
}

/// Windows 查詢程序的結果(純資料,讓判斷邏輯能在所有平台測試)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsProbe {
    /// `OpenProcess` 失敗,帶 `GetLastError()`。
    OpenFailed(u32),
    /// 開得了 handle,`WaitForSingleObject(handle, 0)` 的回傳值。
    Waited(u32),
}

/// 程序存在嗎。刻意不用 `GetExitCodeProcess`:程序真的以 259(STILL_ACTIVE)結束時會被誤判成仍在執行;
/// 程序物件已結束時 wait 會立刻回 `WAIT_OBJECT_0`,沒有這個歧義。
pub fn windows_process_exists(probe: WindowsProbe) -> bool {
    const ERROR_ACCESS_DENIED: u32 = 5;
    const WAIT_TIMEOUT: u32 = 258;
    match probe {
        // 不同使用者或完整性層級的程序開不了:程序存在,只是不屬於自己(對應 unix 的 EPERM)。
        // 其他失敗(ERROR_INVALID_PARAMETER 表示沒有這個 pid)視為不存在。
        WindowsProbe::OpenFailed(error) => error == ERROR_ACCESS_DENIED,
        // 逾時 = 程序物件尚未成為 signaled = 還在跑。
        WindowsProbe::Waited(result) => result == WAIT_TIMEOUT,
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
#[allow(unsafe_code)] // 只有這個函式:開 handle 查詢程序是否已結束。
fn system_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    if pid == 0 {
        return false;
    }
    // SAFETY: 只開 handle、不等待地查詢一次,並關掉自己開的 handle。
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return windows_process_exists(WindowsProbe::OpenFailed(GetLastError()));
        }
        let result = WaitForSingleObject(handle, 0);
        CloseHandle(handle);
        windows_process_exists(WindowsProbe::Waited(result))
    }
}

#[cfg(not(any(unix, windows)))]
fn system_is_alive(_pid: u32) -> bool {
    true
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)] // 只有這個函式:sysconf(_SC_CLK_TCK)。
fn system_started_at_ms(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let boot_time: u64 = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    // SAFETY: sysconf 只查系統常數。
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    linux_start_time_ms(&stat, boot_time, u64::try_from(ticks).ok()?)
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)] // 只有這個函式:proc_pidinfo(PROC_PIDTBSDINFO)。
fn system_started_at_ms(pid: u32) -> Option<u64> {
    let pid = i32::try_from(pid).ok().filter(|pid| *pid > 0)?;
    // SAFETY: 全零是 proc_bsdinfo 的合法值;buffer 大小與型別相符,核心只寫入這塊記憶體。
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let written = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    if written != size {
        return None;
    }
    info.pbi_start_tvsec.checked_mul(1_000)?.checked_add(info.pbi_start_tvusec / 1_000)
}

#[cfg(windows)]
#[allow(unsafe_code)] // 只有這個函式:GetProcessTimes。
fn system_started_at_ms(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    if pid == 0 {
        return None;
    }
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: 只查詢建立時間,四個輸出參數都是本地變數,並關掉自己開的 handle。
    let queried = unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let queried = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0;
        CloseHandle(handle);
        queried
    };
    if !queried {
        return None;
    }
    filetime_to_unix_ms((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

/// 其他平台(BSD 等)取不到啟動時間,退回只看 pid。
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn system_started_at_ms(_pid: u32) -> Option<u64> {
    None
}

/// `running.json` 記載的程序已不存在(且不是自己)時回傳它的內容。
pub fn vanished_previous_run(state_directory: &Path, own_pid: u32, processes: &dyn ProcessTable) -> Option<RunningService> {
    let previous = running_service::read(state_directory)?;
    if previous.pid == own_pid {
        return None;
    }
    // 還活著但啟動時間晚於記載的時間:pid 被別的程序重用,原本那個程序已經不在了。
    let gone = !processes.is_alive(previous.pid) || !same_process(previous.started_at_ms, processes.started_at_ms(previous.pid));
    gone.then_some(previous)
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

/// 服務結束的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Termination {
    /// 收到結束訊號(含原因),是有意的關閉。
    Signal(&'static str),
    /// 接受連線的迴圈自己結束了(panic 或被取消):不是有意的關閉。
    ListenerEnded,
}

/// 記下結束原因並回傳結束碼。訊號是正常關閉:清掉自己的 `running.json`、回 0。
/// 聽連線的迴圈意外結束是失敗:記為錯誤、回 1(讓 systemd `Restart=on-failure` 等監督者重啟),
/// 不清 `running.json`,留給下次啟動的偵測補上「上次未正常關閉」。
pub fn conclude(state_directory: &Path, log: &ServiceLog, termination: Termination) -> u8 {
    match termination {
        Termination::Signal(reason) => {
            record_shutdown(state_directory, log, reason);
            0
        }
        Termination::ListenerEnded => {
            log.record("error: the listener loop ended unexpectedly; exiting with failure");
            1
        }
    }
}

/// 在 runtime 上跑 `future`,結束後最多再等 `grace` 讓 `spawn_blocking` 的工作收尾。
/// runtime 被 drop 時預設會一直等 blocking 工作(herdr CLI、HTTP push、心跳等),卡住的話
/// 優雅返回之後仍會拖到被 SIGKILL;這裡限時,逾時的 blocking 執行緒隨程序結束被丟掉。
pub fn run_with_grace<T>(future: impl std::future::Future<Output = T>, grace: Duration) -> T {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("無法建立 tokio runtime");
    let output = runtime.block_on(future);
    runtime.shutdown_timeout(grace);
    output
}

/// 結束訊號的接收器。必須在啟動工作之前建立:處理器一安裝就攔得住訊號,之後才開始等;
/// 若等到 select 才第一次 poll,啟動窗口內的訊號會被預設動作直接殺掉。
pub struct TerminationSignals(Option<Inner>);

impl TerminationSignals {
    /// 立即註冊處理器(需在 tokio runtime 內呼叫);註冊失敗時 `wait` 會一直等待。
    pub fn install() -> Self {
        Self(Inner::install())
    }

    /// 等到收到結束訊號,回傳原因。
    pub async fn wait(self) -> &'static str {
        match self.0 {
            Some(inner) => inner.wait().await,
            None => std::future::pending().await,
        }
    }
}

#[cfg(unix)]
struct Inner {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
    hangup: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Inner {
    fn install() -> Option<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Some(Self {
            interrupt: signal(SignalKind::interrupt()).ok()?,
            terminate: signal(SignalKind::terminate()).ok()?,
            hangup: signal(SignalKind::hangup()).ok()?,
        })
    }

    async fn wait(mut self) -> &'static str {
        tokio::select! {
            _ = self.interrupt.recv() => "ctrl-c (SIGINT)",
            _ = self.terminate.recv() => "terminate (SIGTERM)",
            _ = self.hangup.recv() => "hangup (SIGHUP)",
        }
    }
}

#[cfg(windows)]
struct Inner {
    interrupt: tokio::signal::windows::CtrlC,
    break_key: tokio::signal::windows::CtrlBreak,
    close: tokio::signal::windows::CtrlClose,
    logoff: tokio::signal::windows::CtrlLogoff,
    shutdown: tokio::signal::windows::CtrlShutdown,
}

#[cfg(windows)]
impl Inner {
    fn install() -> Option<Self> {
        use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_logoff, ctrl_shutdown};
        Some(Self {
            interrupt: ctrl_c().ok()?,
            break_key: ctrl_break().ok()?,
            close: ctrl_close().ok()?,
            logoff: ctrl_logoff().ok()?,
            shutdown: ctrl_shutdown().ok()?,
        })
    }

    async fn wait(mut self) -> &'static str {
        tokio::select! {
            _ = self.interrupt.recv() => "ctrl-c",
            _ = self.break_key.recv() => "ctrl-break",
            _ = self.close.recv() => "console close",
            _ = self.logoff.recv() => "user logoff",
            _ = self.shutdown.recv() => "system shutdown",
        }
    }
}

#[cfg(not(any(unix, windows)))]
struct Inner;

#[cfg(not(any(unix, windows)))]
impl Inner {
    fn install() -> Option<Self> {
        None
    }

    async fn wait(self) -> &'static str {
        std::future::pending().await
    }
}
