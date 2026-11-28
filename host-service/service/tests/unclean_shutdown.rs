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

#[test]
fn the_system_process_table_knows_ourselves_and_not_a_missing_pid() {
    assert!(shutdown::SystemProcesses.is_alive(std::process::id()));
    assert!(!shutdown::SystemProcesses.is_alive(2_000_000_000));
    assert!(!shutdown::SystemProcesses.is_alive(0));
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
