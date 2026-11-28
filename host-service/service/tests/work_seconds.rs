//! 工作時間(raychiutw/purrdr.host#96):done 推播的選用欄位 `workSeconds` 是 pane 進入 working 到 done 的整數秒。
//! 重啟後沒有開始時間、時間倒退(亂序)都省略欄位。以明確的時間點驅動，不實際等待。

use std::time::{Duration, Instant};

use purrdr_host_service::attention::AttentionDetector;

const MINUTE: Duration = Duration::from_secs(60);

fn detector() -> AttentionDetector {
    AttentionDetector::default()
}

#[test]
fn finishing_work_reports_whole_seconds_since_the_pane_entered_working() {
    let start = Instant::now();
    let mut detector = detector();
    detector.status("w1:pA", "working", start);

    assert_eq!(detector.finish_work("w1:pA", start + Duration::from_millis(754_900)), Some(754));
    // 回報過一次就用掉了:同一段期間不會重複報。
    assert_eq!(detector.finish_work("w1:pA", start + 800 * Duration::from_secs(1)), None);
}

#[test]
fn finishing_work_without_a_recorded_start_reports_nothing() {
    let start = Instant::now();
    let mut detector = detector();

    assert_eq!(detector.finish_work("w1:pA", start), None);
    // 只看過 blocked、沒看過進入 working:沒有開始時間。
    detector.status("w1:pA", "blocked", start);
    assert_eq!(detector.finish_work("w1:pA", start + MINUTE), None);
}

#[test]
fn a_working_pane_found_only_by_a_snapshot_has_no_start_time() {
    // 主機服務重啟(或訂閱斷線)後第一次看到 pane 已在 working:真正的開始時間不知道，不能拿「現在」當開始。
    let start = Instant::now();
    let mut detector = detector();
    detector.snapshot_status("w1:pA", "working", start);

    assert_eq!(detector.finish_work("w1:pA", start + 5 * MINUTE), None);
}

#[test]
fn a_snapshot_does_not_erase_a_start_time_learned_from_an_event() {
    let start = Instant::now();
    let mut detector = detector();
    detector.status("w1:pA", "working", start);
    detector.snapshot_status("w1:pA", "working", start + MINUTE);

    assert_eq!(detector.finish_work("w1:pA", start + 3 * MINUTE), Some(180));
}

#[test]
fn an_earlier_finish_time_than_the_start_reports_nothing() {
    // 時間倒退(事件亂序):不報負數也不報 0,直接省略。
    let start = Instant::now();
    let mut detector = detector();
    detector.status("w1:pA", "working", start + 10 * MINUTE);

    assert_eq!(detector.finish_work("w1:pA", start), None);
}

#[test]
fn work_time_ends_when_the_pane_becomes_blocked_so_done_after_blocked_reports_nothing() {
    // 與外掛一致:blocked 是一段工作的結束,等使用者的時間不算進去。
    let start = Instant::now();
    let mut detector = detector();
    detector.status("w1:pA", "working", start);
    detector.status("w1:pA", "blocked", start + MINUTE);

    assert_eq!(detector.finish_work("w1:pA", start + 30 * MINUTE), None);
}

#[test]
fn work_time_restarts_when_a_blocked_pane_resumes_and_clears_when_it_goes_idle() {
    let start = Instant::now();
    let mut detector = detector();
    detector.status("w1:pA", "working", start);
    detector.status("w1:pA", "blocked", start + MINUTE);
    detector.status("w1:pA", "working", start + 10 * MINUTE);
    assert_eq!(detector.finish_work("w1:pA", start + 12 * MINUTE), Some(120));

    detector.status("w1:pB", "working", start);
    detector.status("w1:pB", "idle", start + MINUTE);
    assert_eq!(detector.finish_work("w1:pB", start + 2 * MINUTE), None);
}
