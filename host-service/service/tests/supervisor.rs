//! Windows 看門狗的重啟策略(#112):間隔、次數上限、停止標記。

use std::io;
use std::time::Duration;

use purrdr_host_service::supervisor::{ChildExit, Outcome, Policy, Supervised, supervise};

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
    purrdr_host_service::supervisor::request_stop(&directory);
    assert!(purrdr_host_service::supervisor::stop_marker(&directory).is_file());
    purrdr_host_service::supervisor::clear_stop(&directory);
    assert!(!purrdr_host_service::supervisor::stop_marker(&directory).exists());
    let _ = std::fs::remove_dir_all(&directory);
}
