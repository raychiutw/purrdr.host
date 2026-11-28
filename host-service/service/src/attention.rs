//! 卡住與仍在等你的判定(raychiutw/purrdr.host#31)。
//!
//! 每個 pane 一個「狀態期間」(狀態真的改變時開始):working 期間畫面指紋連續 `stuck_after` 不變是可能卡住，
//! blocked 持續 `reminder_after` 是仍在等你。每個期間最多提醒一次。時間由呼叫端傳入，判定本身不讀時鐘。

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// 畫面不變多久算可能卡住。
pub const STUCK_AFTER: Duration = Duration::from_secs(10 * 60);
/// 停在「需要你」多久再提醒。
pub const REMINDER_AFTER: Duration = Duration::from_secs(15 * 60);

/// working 期間的提醒(仍在等你由 `reminders` 回報)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alert {
    Stuck,
}

#[derive(Debug)]
struct Period {
    status: String,
    since: Instant,
    /// 最近一次的畫面指紋與它從何時開始沒變。
    screen: Option<(String, Instant)>,
    alerted: bool,
}

/// 各 pane 的狀態期間。
#[derive(Debug)]
pub struct AttentionDetector {
    pub stuck_after: Duration,
    pub reminder_after: Duration,
    periods: BTreeMap<String, Period>,
    /// 各 pane 最近一次(由事件得知)進入 working 的時間(進入 blocked／idle／done 即清掉),供 done 推播算 `workSeconds`(raychiutw/purrdr.host#96);只存在記憶體,重啟後為空。
    working_since: BTreeMap<String, Instant>,
}

impl Default for AttentionDetector {
    fn default() -> Self {
        AttentionDetector::new(STUCK_AFTER, REMINDER_AFTER)
    }
}

impl AttentionDetector {
    pub fn new(stuck_after: Duration, reminder_after: Duration) -> Self {
        AttentionDetector {
            stuck_after,
            reminder_after,
            periods: BTreeMap::new(),
            working_since: BTreeMap::new(),
        }
    }

    /// herdr 回報的狀態;跟目前一樣時不是新期間。
    pub fn status(&mut self, pane_id: &str, status: &str, now: Instant) {
        self.update(pane_id, status, now, true);
    }

    fn update(&mut self, pane_id: &str, status: &str, now: Instant, from_event: bool) {
        // 與外掛一致:blocked 也是一段工作的結束(等使用者的時間不算工作),done 前若是 blocked 就不帶 workSeconds。
        if matches!(status, "idle" | "done" | "blocked") {
            self.working_since.remove(pane_id);
        }
        if self.periods.get(pane_id).is_some_and(|period| period.status == status) {
            // 事件回報 working 而 snapshot 先前已讓期間是 working:事件才是進入的證據,補記開始時間(已有就不動)。
            if from_event && status == "working" {
                self.working_since.entry(pane_id.to_owned()).or_insert(now);
            }
            return;
        }
        if status == "working" {
            // 只有事件知道真正的進入時間;snapshot 才看到的轉換不留舊值(那會高報)。
            if from_event {
                self.working_since.insert(pane_id.to_owned(), now);
            } else {
                self.working_since.remove(pane_id);
            }
        }
        self.periods.insert(
            pane_id.to_owned(),
            Period {
                status: status.to_owned(),
                since: now,
                screen: None,
                alerted: false,
            },
        );
    }

    /// working 的 pane 讀到的畫面指紋;這次判定為可能卡住時回 `Stuck`。
    pub fn screen(&mut self, pane_id: &str, fingerprint: &str, now: Instant) -> Option<Alert> {
        let period = self.periods.get_mut(pane_id).filter(|period| period.status == "working")?;
        match &period.screen {
            Some((previous, _)) if previous == fingerprint => {}
            _ => {
                period.screen = Some((fingerprint.to_owned(), now));
                return None;
            }
        }
        let unchanged_since = period.screen.as_ref().map(|(_, since)| *since)?;
        if period.alerted || now.saturating_duration_since(unchanged_since) < self.stuck_after {
            return None;
        }
        period.alerted = true;
        Some(Alert::Stuck)
    }

    /// 讀 snapshot 對帳得到的狀態:同 `status`,但不把「現在」當成進入 working 的時間
    /// (重啟或斷線後才看到 pane 在 working,真正的開始時間不知道,done 推播就不帶 `workSeconds`)。
    pub fn snapshot_status(&mut self, pane_id: &str, status: &str, now: Instant) {
        self.update(pane_id, status, now, false);
    }

    /// pane 做完時回報工作秒數(進入 working 到 `now`,無條件捨去)並用掉這個開始時間。
    /// 沒有開始時間或 `now` 早於開始時間(事件亂序)時回 None。要在 `status(done)` 之前呼叫。
    pub fn finish_work(&mut self, pane_id: &str, now: Instant) -> Option<u64> {
        let since = self.working_since.remove(pane_id)?;
        now.checked_duration_since(since).map(|elapsed| elapsed.as_secs())
    }

    /// 這次判定為仍在等你的 pane。
    pub fn reminders(&mut self, now: Instant) -> Vec<String> {
        let mut due = Vec::new();
        for (pane_id, period) in &mut self.periods {
            if period.status == "blocked" && !period.alerted && now.saturating_duration_since(period.since) >= self.reminder_after {
                period.alerted = true;
                due.push(pane_id.clone());
            }
        }
        due
    }

    /// 需要讀畫面的 pane(working 且這期間還沒提醒過)。
    pub fn working_panes(&self) -> Vec<String> {
        self.periods
            .iter()
            .filter(|(_, period)| period.status == "working" && !period.alerted)
            .map(|(pane_id, _)| pane_id.clone())
            .collect()
    }
}
