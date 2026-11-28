//! Windows 看門狗的重啟策略(#112):間隔、次數上限、停止標記。

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use purrdr_host_service::shutdown::ProcessTable;
use purrdr_host_service::supervisor::{self, ChildExit, Outcome, Policy, Supervised, WatchdogRecord, WatchdogState, supervise};

const SECOND: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Fake {
    exits: Vec<io::Result<ChildExit>>,
    /// 第幾次 `stop_requested` 查詢起回 true(0 起算);None 為從不停止。
    stop_after_checks: Option<usize>,
    checks: usize,
    launches: usize,
    sleeps: Vec<Duration>,
    logs: Vec<String>,
}

impl Fake {
    fn exiting(exits: Vec<(Option<i32>, u64)>) -> Fake {
        Fake {
            exits: exits
                .into_iter()
                .rev()
                .map(|(code, secs)| {
                    Ok(ChildExit {
                        code,
                        lived: Duration::from_secs(secs),
                    })
                })
                .collect(),
            ..Fake::default()
        }
    }
}

impl Supervised for Fake {
    fn run_child(&mut self) -> io::Result<ChildExit> {
        self.launches += 1;
        self.exits.pop().expect("測試沒有準備足夠的結束結果")
    }
    fn stop_requested(&mut self) -> bool {
        self.checks += 1;
        self.stop_after_checks.is_some_and(|limit| self.checks > limit)
    }
    fn sleep(&mut self, duration: Duration) {
        self.sleeps.push(duration);
    }
    fn log(&mut self, message: &str) {
        self.logs.push(message.to_owned());
    }
}

#[test]
fn default_policy_has_bounded_backoff() {
    let policy = Policy::default();
    assert_eq!(policy.base_delay, 5 * SECOND);
    assert_eq!(policy.max_delay, 300 * SECOND);
    assert_eq!(policy.healthy_after, 60 * SECOND);
    assert_eq!(policy.max_consecutive_failures, 10);
}

#[test]
fn restarts_a_vanished_child_after_the_base_delay() {
    let mut fake = Fake::exiting(vec![(Some(1), 3600), (Some(1), 3600)]);
    // 每次結束後檢查一次、等待後再檢查一次;第 4 次檢查(第二次等待後)起要求停止。
    fake.stop_after_checks = Some(3);

    let outcome = supervise(&mut fake, &Policy::default());

    assert_eq!(outcome, Outcome::StopRequested);
    assert_eq!(fake.launches, 2);
    assert_eq!(fake.sleeps, vec![5 * SECOND, 5 * SECOND], "子程序活得夠久就不累積退避");
    assert!(fake.logs.iter().any(|line| line.contains("restarting")), "{:?}", fake.logs);
}

#[test]
fn quick_failures_back_off_exponentially_up_to_the_cap_then_give_up() {
    let mut fake = Fake::exiting(vec![(Some(1), 1); 10]);

    let outcome = supervise(&mut fake, &Policy::default());

    assert_eq!(outcome, Outcome::GaveUp { failures: 10 });
    assert_eq!(fake.launches, 10);
    let seconds: Vec<u64> = fake.sleeps.iter().map(Duration::as_secs).collect();
    assert_eq!(seconds, vec![5, 10, 20, 40, 80, 160, 300, 300, 300]);
    assert!(fake.logs.last().unwrap().contains("giving up"), "{:?}", fake.logs);
}

#[test]
fn a_healthy_run_resets_the_failure_count() {
    let mut exits = vec![(Some(1), 1); 9];
    exits.push((Some(1), 120));
    exits.extend(vec![(Some(1), 1); 10]);
    let mut fake = Fake::exiting(exits);

    let outcome = supervise(&mut fake, &Policy::default());

    assert_eq!(outcome, Outcome::GaveUp { failures: 10 });
    assert_eq!(fake.launches, 20);
}

#[test]
fn usage_error_exit_is_not_restarted() {
    let mut fake = Fake::exiting(vec![(Some(2), 0)]);

    assert_eq!(supervise(&mut fake, &Policy::default()), Outcome::UsageError);
    assert_eq!(fake.launches, 1);
    assert!(fake.sleeps.is_empty());
}

#[test]
fn stop_marker_present_after_the_child_exits_prevents_restart() {
    let mut fake = Fake::exiting(vec![(Some(1), 5)]);
    fake.stop_after_checks = Some(0);

    assert_eq!(supervise(&mut fake, &Policy::default()), Outcome::StopRequested);
    assert_eq!(fake.launches, 1);
    assert!(fake.sleeps.is_empty(), "要停止就不必等退避");
}

#[test]
fn stop_marker_appearing_during_the_backoff_wait_prevents_restart() {
    let mut fake = Fake::exiting(vec![(Some(1), 5), (Some(1), 5)]);
    // 子程序結束後的檢查沒有標記,等退避之後的檢查才有。
    fake.stop_after_checks = Some(1);

    assert_eq!(supervise(&mut fake, &Policy::default()), Outcome::StopRequested);
    assert_eq!(fake.launches, 1);
}

#[test]
fn a_child_that_cannot_be_launched_counts_as_a_failure() {
    let mut fake = Fake {
        exits: (0..10).map(|_| Err(io::Error::other("cannot start"))).collect(),
        ..Fake::default()
    };

    assert_eq!(supervise(&mut fake, &Policy::default()), Outcome::GaveUp { failures: 10 });
}

#[test]
fn stop_marker_helpers_create_and_clear_the_file() {
    let directory = std::env::temp_dir().join(format!("purrdr-supervisor-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    purrdr_host_service::supervisor::request_stop(&directory).unwrap();
    assert!(purrdr_host_service::supervisor::stop_marker(&directory).is_file());
    purrdr_host_service::supervisor::clear_stop(&directory);
    assert!(!purrdr_host_service::supervisor::stop_marker(&directory).exists());
    let _ = std::fs::remove_dir_all(&directory);
}

// --- 第一輪 code review 修正(#113) ---

#[test]
fn clean_exit_after_a_signal_shutdown_is_treated_as_an_intentional_stop() {
    // 登出、關機、console close 訊號讓服務走正常關閉(結束碼 0):是有意停止,看門狗不該把它拉回來。
    let mut fake = Fake::exiting(vec![(Some(0), 3600)]);

    assert_eq!(supervise(&mut fake, &Policy::default()), Outcome::StopRequested);
    assert_eq!(fake.launches, 1);
    assert!(fake.sleeps.is_empty());
}

#[test]
fn watchdog_ties_the_child_to_its_own_lifetime_with_a_job_object() {
    // 只能在 Windows 驗證行為;這裡守住「有設 KILL_ON_JOB_CLOSE 並且 windows-sys 開了對應 feature」。
    let source = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/supervisor.rs")).unwrap();
    let manifest = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    assert!(source.contains("JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE"));
    assert!(manifest.contains("Win32_System_JobObjects"));
}

#[test]
fn request_stop_reports_when_the_marker_cannot_be_written() {
    let directory = std::env::temp_dir().join(format!("purrdr-supervisor-stop-failure-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(supervisor::stop_marker(&directory)).unwrap(); // 標記的位置被資料夾佔住,寫不進去

    assert!(supervisor::request_stop(&directory).is_err());

    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn waiting_for_the_old_processes_stops_as_soon_as_they_are_gone() {
    let mut checks = 0;
    let mut sleeps = 0;
    let gone = supervisor::wait_until_gone(
        || {
            checks += 1;
            checks < 4
        },
        |_| sleeps += 1,
        Duration::from_millis(100),
        50,
    );
    assert!(gone);
    assert_eq!((checks, sleeps), (4, 3));
}

#[test]
fn waiting_for_the_old_processes_gives_up_after_the_attempt_limit() {
    let mut sleeps = 0;
    let gone = supervisor::wait_until_gone(|| true, |_| sleeps += 1, Duration::from_millis(100), 5);
    assert!(!gone);
    assert_eq!(sleeps, 5);
}

#[test]
fn state_directory_resolution_explains_why_supervision_is_disabled() {
    let arguments = |texts: &[&str]| texts.iter().map(|text| (*text).to_owned()).collect::<Vec<_>>();

    assert_eq!(
        supervisor::resolve_state_directory(&arguments(&["--state-dir", "/data"]), || None),
        Ok(PathBuf::from("/data"))
    );
    assert_eq!(
        supervisor::resolve_state_directory(&arguments(&["--port", "1"]), || Some(PathBuf::from("/home/x/.purrdr/host-service"))),
        Ok(PathBuf::from("/home/x/.purrdr/host-service"))
    );
    let missing_value = supervisor::resolve_state_directory(&arguments(&["--state-dir"]), || Some(PathBuf::from("/default"))).unwrap_err();
    assert!(missing_value.contains("--state-dir"), "{missing_value}");
    let no_home = supervisor::resolve_state_directory(&arguments(&[]), || None).unwrap_err();
    assert!(no_home.contains("home"), "{no_home}");
}

struct FakeProcesses {
    alive: bool,
}

impl ProcessTable for FakeProcesses {
    fn is_alive(&self, _pid: u32) -> bool {
        self.alive
    }
}

#[test]
fn watchdog_note_reflects_the_actual_state() {
    let record = |state| WatchdogRecord {
        pid: 10,
        state,
        started_at_ms: 1_000,
    };
    let alive = FakeProcesses { alive: true };
    let gone = FakeProcesses { alive: false };

    assert!(supervisor::watchdog_note(Some(&record(WatchdogState::Running)), &alive).starts_with("watchdog running"));
    assert!(supervisor::watchdog_note(Some(&record(WatchdogState::Running)), &gone).starts_with("watchdog not running"));
    assert!(supervisor::watchdog_note(None, &alive).starts_with("watchdog not running"));
    assert!(supervisor::watchdog_note(Some(&record(WatchdogState::GaveUp)), &gone).starts_with("watchdog gave up"));
}

#[test]
fn watchdog_record_round_trips_through_the_state_directory() {
    let directory = std::env::temp_dir().join(format!("purrdr-supervisor-record-{}", std::process::id()));
    let record = WatchdogRecord {
        pid: 77,
        state: WatchdogState::GaveUp,
        started_at_ms: 5,
    };
    supervisor::write_watchdog(&directory, &record).unwrap();
    assert_eq!(supervisor::read_watchdog(&directory), Some(record));
    supervisor::remove_watchdog(&directory);
    assert_eq!(supervisor::read_watchdog(&directory), None);
    let _ = std::fs::remove_dir_all(&directory);
}
