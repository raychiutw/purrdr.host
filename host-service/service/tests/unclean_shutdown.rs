//! 異常結束要留下證據(raychiutw/purrdr.host#109):未正常關閉的偵測、關閉原因紀錄、`purrdr status` 顯示。

mod install_support;

use std::collections::HashSet;
use std::path::Path;

use install_support::{FakeSystem, TempDirectory, TestHost, reserved_port};
use purrdr_host_service::herdr_socket::SocketEnvironment;
use purrdr_host_service::install::{InstallPlatform, Installer};
use purrdr_host_service::running_service::{self, RunningService};
use purrdr_host_service::service_log::ServiceLog;
use purrdr_host_service::shutdown::{self, ProcessTable};

/// 只有列出的 pid 還活著。
struct Alive(HashSet<u32>);

impl ProcessTable for Alive {
    fn is_alive(&self, pid: u32) -> bool {
        self.0.contains(&pid)
    }
}

fn nobody() -> Alive {
    Alive(HashSet::new())
}

fn write_marker(directory: &Path, pid: u32) {
    let marker = RunningService {
        version: "0.17.0".into(),
        port: 47_820,
        pid,
        // 2026-11-28T10:00:00Z
        started_at_ms: 1_795_860_000_000,
    };
    std::fs::create_dir_all(directory).unwrap();
    std::fs::write(directory.join("running.json"), serde_json::to_string(&marker).unwrap()).unwrap();
}

fn log_text(directory: &Path) -> String {
    std::fs::read_to_string(directory.join("logs").join("host-service.log")).unwrap_or_default()
}

fn missing_socket() -> SocketEnvironment {
    SocketEnvironment {
        socket_path: Some("purrdr-unclean-test-missing-socket".into()),
        ..SocketEnvironment::default()
    }
}

#[test]
fn a_vanished_previous_process_is_logged_once_with_pid_version_and_start_time() {
    let state = TempDirectory::new("unclean-once");
    write_marker(&state.0, 4242);
    let log = ServiceLog::under(&state.0);

    shutdown::report_unclean_previous_run(&state.0, &log, 100, &nobody());
    shutdown::report_unclean_previous_run(&state.0, &log, 100, &nobody());

    let text = log_text(&state.0);
    assert_eq!(text.matches("did not shut down cleanly").count(), 1, "{text}");
    assert!(text.contains("pid 4242"), "{text}");
    assert!(text.contains("version 0.17.0"), "{text}");
    assert!(text.contains("started 2026-11-28T10:00:00Z"), "{text}");
}

#[test]
fn a_previous_process_that_is_still_alive_or_is_ourselves_is_not_reported() {
    let state = TempDirectory::new("unclean-alive");
    let log = ServiceLog::under(&state.0);
    write_marker(&state.0, 4242);
    shutdown::report_unclean_previous_run(&state.0, &log, 100, &Alive(HashSet::from([4242])));
    write_marker(&state.0, 100);
    shutdown::report_unclean_previous_run(&state.0, &log, 100, &nobody());

    assert_eq!(log_text(&state.0), "");
}

#[test]
fn no_running_json_means_nothing_to_report() {
    let state = TempDirectory::new("unclean-none");

    shutdown::report_unclean_previous_run(&state.0, &ServiceLog::under(&state.0), 100, &nobody());

    assert_eq!(log_text(&state.0), "");
}

#[test]
fn a_clean_shutdown_records_the_reason_and_the_next_start_reports_nothing() {
    let state = TempDirectory::new("unclean-clean");
    let log = ServiceLog::under(&state.0);
    running_service::write(&state.0, 47_999).unwrap();

    shutdown::record_shutdown(&state.0, &log, "terminate (SIGTERM)");
    shutdown::report_unclean_previous_run(&state.0, &log, 100, &nobody());

    let text = log_text(&state.0);
    assert!(text.contains("stopping: terminate (SIGTERM)"), "{text}");
    assert!(!text.contains("did not shut down cleanly"), "{text}");
    assert!(running_service::read(&state.0).is_none());
}

#[test]
fn shutdown_leaves_another_processes_running_json_alone() {
    let state = TempDirectory::new("unclean-other");
    write_marker(&state.0, std::process::id().wrapping_add(1));

    shutdown::record_shutdown(&state.0, &ServiceLog::under(&state.0), "ctrl-c");

    assert!(running_service::read(&state.0).is_some());
}

/// 指定 pid 還活著、而且程序是在某個時間點啟動的。
struct AliveSince(u32, Option<u64>);

impl ProcessTable for AliveSince {
    fn is_alive(&self, pid: u32) -> bool {
        pid == self.0
    }

    fn started_at_ms(&self, _pid: u32) -> Option<u64> {
        self.1
    }
}

const MARKER_STARTED_AT_MS: u64 = 1_795_860_000_000;

#[test]
fn a_reused_pid_that_started_after_the_recorded_run_is_reported() {
    let state = TempDirectory::new("unclean-reused");
    write_marker(&state.0, 4242);
    let log = ServiceLog::under(&state.0);

    // 斷電重開機後 pid 被別的程序拿去用:它的啟動時間比 running.json 記的晚得多。
    shutdown::report_unclean_previous_run(&state.0, &log, 100, &AliveSince(4242, Some(MARKER_STARTED_AT_MS + 600_000)));

    let text = log_text(&state.0);
    assert!(text.contains("did not shut down cleanly") && text.contains("pid 4242"), "{text}");
}

#[test]
fn a_live_process_that_started_before_the_recorded_run_or_with_unknown_start_is_not_reported() {
    let state = TempDirectory::new("unclean-same");
    let log = ServiceLog::under(&state.0);
    for started in [Some(MARKER_STARTED_AT_MS - 5_000), Some(MARKER_STARTED_AT_MS + 1_000), None] {
        write_marker(&state.0, 4242);
        shutdown::report_unclean_previous_run(&state.0, &log, 100, &AliveSince(4242, started));
    }

    assert_eq!(log_text(&state.0), "");
}

#[test]
fn process_start_times_are_compared_with_a_small_tolerance() {
    assert!(shutdown::same_process(10_000, None));
    assert!(shutdown::same_process(10_000, Some(3_000)));
    assert!(shutdown::same_process(10_000, Some(11_999)));
    assert!(!shutdown::same_process(10_000, Some(12_001)));
}

#[test]
fn linux_proc_stat_start_time_is_boot_time_plus_ticks() {
    // comm 含空白與括號時,以最後一個 ')' 為界;第 22 欄 starttime 是開機後的 clock ticks。
    let stat = "4242 (my (odd) app) S 1 4242 4242 0 -1 4194560 100 0 0 0 5 3 0 0 20 0 1 0 25000 1000000 100 18446744073709551615";
    assert_eq!(shutdown::linux_start_time_ms(stat, 1_000, 100), Some(1_000_000 + 250_000));
    assert_eq!(shutdown::linux_start_time_ms("garbage", 1_000, 100), None);
    assert_eq!(shutdown::linux_start_time_ms(stat, 1_000, 0), None);
}

#[test]
fn windows_filetime_converts_to_unix_milliseconds() {
    // 1970-01-01T00:00:00Z 是 FILETIME 116_444_736_000_000_000。
    assert_eq!(shutdown::filetime_to_unix_ms(116_444_736_000_000_000), Some(0));
    assert_eq!(shutdown::filetime_to_unix_ms(116_444_736_000_000_000 + 10_000 * 1_500), Some(1_500));
    assert_eq!(shutdown::filetime_to_unix_ms(0), None);
}

#[test]
fn windows_probe_results_map_to_alive_or_gone() {
    use shutdown::WindowsProbe::{OpenFailed, Waited};
    // WaitForSingleObject(handle, 0):WAIT_TIMEOUT(258)還在跑,WAIT_OBJECT_0(0)已結束。
    assert!(shutdown::windows_process_exists(Waited(258)));
    assert!(!shutdown::windows_process_exists(Waited(0)));
    assert!(!shutdown::windows_process_exists(Waited(0xFFFF_FFFF)));
    // ERROR_ACCESS_DENIED(5):程序存在,只是不屬於自己(對應 unix 的 EPERM)。
    assert!(shutdown::windows_process_exists(OpenFailed(5)));
    // ERROR_INVALID_PARAMETER(87):沒有這個 pid。
    assert!(!shutdown::windows_process_exists(OpenFailed(87)));
    assert!(!shutdown::windows_process_exists(OpenFailed(0)));
}

#[test]
fn a_listener_loop_that_ends_is_a_failure_and_leaves_running_json_for_the_next_start() {
    let state = TempDirectory::new("unclean-listener-ended");
    let log = ServiceLog::under(&state.0);
    running_service::write(&state.0, 47_999).unwrap();

    let code = shutdown::conclude(&state.0, &log, shutdown::Termination::ListenerEnded);

    assert_eq!(code, 1);
    let text = log_text(&state.0);
    assert!(text.contains("listener loop ended unexpectedly"), "{text}");
    assert!(!text.contains("stopping:"), "{text}");
    assert!(running_service::read(&state.0).is_some());
}

#[test]
fn a_signal_is_a_clean_shutdown_with_exit_code_zero() {
    let state = TempDirectory::new("unclean-signal-code");
    running_service::write(&state.0, 47_999).unwrap();

    let code = shutdown::conclude(&state.0, &ServiceLog::under(&state.0), shutdown::Termination::Signal("ctrl-c"));

    assert_eq!(code, 0);
    assert!(running_service::read(&state.0).is_none());
}

#[test]
fn exiting_does_not_wait_for_a_stuck_blocking_task() {
    let started = std::time::Instant::now();

    let value = shutdown::run_with_grace(
        async {
            // 模擬卡住的 herdr CLI／HTTP push:blocking 執行緒不會自己結束。
            tokio::task::spawn_blocking(|| std::thread::sleep(std::time::Duration::from_secs(30)));
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            7
        },
        std::time::Duration::from_millis(300),
    );

    assert_eq!(value, 7);
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
}

#[test]
fn the_system_process_table_knows_ourselves_and_not_a_missing_pid() {
    assert!(shutdown::SystemProcesses.is_alive(std::process::id()));
    assert!(!shutdown::SystemProcesses.is_alive(2_000_000_000));
    assert!(!shutdown::SystemProcesses.is_alive(0));
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
#[test]
fn the_system_process_table_knows_when_our_own_process_started() {
    let started = shutdown::SystemProcesses.started_at_ms(std::process::id()).expect("本平台應取得啟動時間");
    let now = running_service::unix_millis(std::time::SystemTime::now());
    // 測試程序是剛啟動的:啟動時間在過去 10 分鐘內(容許 2 秒時鐘誤差)。
    assert!(started <= now + 2_000 && now - started.min(now) < 600_000, "started {started}, now {now}");
    assert_eq!(shutdown::SystemProcesses.started_at_ms(2_000_000_000), None);
}

#[tokio::test]
async fn status_says_the_last_process_is_gone() {
    let host = TestHost::new(InstallPlatform::current());
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    installer.install(&host.options(reserved_port()), |_| true).unwrap();
    write_marker(&host.service_directory(), 2_000_000_000);

    let report = installer.status(&missing_socket()).await.to_string();

    assert!(
        report.contains("  last run: process is gone, not shut down cleanly (pid 2000000000, version 0.17.0, started 2026-11-28T10:00:00Z)\n"),
        "{report}"
    );
}

#[tokio::test]
async fn status_says_nothing_when_the_recorded_process_is_alive() {
    let host = TestHost::new(InstallPlatform::current());
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    installer.install(&host.options(reserved_port()), |_| true).unwrap();
    write_marker(&host.service_directory(), std::process::id());

    let report = installer.status(&missing_socket()).await.to_string();

    assert!(!report.contains("last run"), "{report}");
}

#[cfg(unix)]
mod real_process {
    use super::*;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    fn start(state: &TempDirectory) -> Child {
        Command::new(env!("CARGO_BIN_EXE_purrdr-host-service"))
            .args(["--port", "0", "--state-dir"])
            .arg(&state.0)
            .env("HERDR_SOCKET_PATH", "purrdr-unclean-missing-socket")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    #[test]
    #[allow(unsafe_code)] // 對自己啟動的子程序送 SIGTERM。
    fn sigterm_during_the_startup_window_is_still_a_clean_shutdown() {
        let state = TempDirectory::new("unclean-early-signal");
        // running.json 寫好之後、進入等待之前還有一段啟動工作;這裡拉長它,讓訊號一定落在窗口內。
        let mut child = Command::new(env!("CARGO_BIN_EXE_purrdr-host-service"))
            .args(["--port", "0", "--state-dir"])
            .arg(&state.0)
            .env("HERDR_SOCKET_PATH", "purrdr-unclean-missing-socket")
            .env("PURRDR_TEST_STARTUP_DELAY_MS", "4000")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_until("running.json", || running_service::read(&state.0).is_some_and(|run| run.pid == child.id()));

        unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        let status = child.wait().unwrap();

        let text = log_text(&state.0);
        assert!(status.success(), "{status:?}\n{text}");
        assert!(text.contains("stopping: terminate (SIGTERM)"), "{text}");
        assert!(running_service::read(&state.0).is_none());
    }

    fn wait_until(what: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !condition() {
            assert!(Instant::now() < deadline, "等不到：{what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    #[allow(unsafe_code)] // 對自己啟動的子程序送 SIGTERM。
    fn sigterm_logs_the_reason_and_removes_running_json_and_sigkill_is_detected_on_the_next_start() {
        let state = TempDirectory::new("unclean-real");

        let mut terminated = start(&state);
        wait_until("running.json", || running_service::read(&state.0).is_some_and(|run| run.pid == terminated.id()));
        unsafe { libc::kill(terminated.id() as i32, libc::SIGTERM) };
        let status = terminated.wait().unwrap();
        assert!(status.success(), "{status:?}");
        let text = log_text(&state.0);
        assert!(text.contains("stopping: terminate (SIGTERM)"), "{text}");
        assert!(running_service::read(&state.0).is_none());

        let mut killed = start(&state);
        wait_until("running.json", || running_service::read(&state.0).is_some_and(|run| run.pid == killed.id()));
        let killed_pid = killed.id();
        killed.kill().unwrap();
        killed.wait().unwrap();
        let mut restarted = start(&state);
        wait_until("report", || log_text(&state.0).contains("did not shut down cleanly"));
        let _ = restarted.kill();
        let _ = restarted.wait();

        let text = log_text(&state.0);
        assert_eq!(text.matches("did not shut down cleanly").count(), 1, "{text}");
        assert!(text.contains(&format!("pid {killed_pid}")), "{text}");
    }
}
