//! 方案用量推播(raychiutw/purrdr.host#98,ADR-0016／0017):Claude 與 Codex 的週期用量跨過 80%／95% 時，
//! 對每支配對的裝置送 `{"key","status":"usage","agent","threshold","resetsAt"}`(不帶 workspace、pane、tab;resetsAt 是 Unix 秒)。
//!
//! - 跨過幾個門檻就各送一則;每個(agent、週期種類、resetsAt、門檻)主機端只送一次，週期重置(resetsAt 變了)後可再送。
//!   記憶只放在記憶體:主機服務重啟後可能重送，後台以 (會員, agent, resetsAt, threshold) 去重。
//! - 沒有重置時間就不送(後台無從去重)。
//! - Claude 讀 status line 用量檔(便宜,每個檢查週期都讀);Codex 要起 `codex app-server`,成本高:
//!   配對檔只有 endpoint／key／muted,看不出後台開了哪些種類，所以保守作法是「至少有一組配對才讀」並限制頻率。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use tokio::task::JoinHandle;

use crate::claude_usage::ClaudeUsage;
use crate::codex_usage::{self, CodexCli};
use crate::protocol::{PlanUsagePeriod, PlanUsagePeriodKind};
use crate::update_push::{HttpPushTransport, PushTransport, pairings};

/// 要通知的用量門檻(百分比)。
pub const THRESHOLDS: [u8; 2] = [80, 95];
/// 後台：這支裝置的配對已撤銷。
const REVOKED: u16 = 410;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsageAgent {
    Claude,
    Codex,
}

impl UsageAgent {
    fn name(self) -> &'static str {
        match self {
            UsageAgent::Claude => "claude",
            UsageAgent::Codex => "codex",
        }
    }
}

/// 某個 Agent 某個週期的讀數。
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub agent: UsageAgent,
    pub kind: PlanUsagePeriodKind,
    pub used_percentage: f64,
    /// 週期重置時間(Unix 秒);沒有就不送。
    pub resets_at: Option<u64>,
}

/// 把協定的週期(重置時間為 RFC 3339)轉成讀數。
pub fn readings_from(agent: UsageAgent, periods: &[PlanUsagePeriod]) -> Vec<Reading> {
    periods
        .iter()
        .map(|period| Reading {
            agent,
            kind: period.kind,
            used_percentage: period.used_percentage,
            resets_at: period
                .resets_at
                .as_deref()
                .and_then(|text| humantime::parse_rfc3339(text).ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|elapsed| elapsed.as_secs()),
        })
        .collect()
}

/// 讀 Claude 的 status line 用量檔(沒有檔案、已過重置時間的週期都沒有讀數)。
pub fn claude_usage_reading(home: &Path) -> Vec<Reading> {
    match ClaudeUsage::new(Some(home.to_owned())).plan_usage() {
        Ok(usage) => readings_from(UsageAgent::Claude, &usage.periods),
        Err(_) => Vec::new(),
    }
}

/// 記住已送過的(agent、週期種類、resetsAt、門檻)。
#[derive(Default)]
pub struct UsageNotifier {
    sent: HashSet<(UsageAgent, PlanUsagePeriodKind, u64, u8)>,
}

impl UsageNotifier {
    pub fn new() -> Self {
        UsageNotifier::default()
    }

    /// 對每個跨過門檻、還沒送過的讀數送一則;沒有配對、連不上都不記成已送(之後再試)。
    pub fn notify(&mut self, home: &Path, transport: &mut dyn PushTransport, readings: &[Reading]) {
        let pairings = pairings(&home.join(".purrdr"));
        if pairings.is_empty() {
            return;
        }
        for reading in readings {
            let Some(resets_at) = reading.resets_at else { continue };
            // 週期重置了:同種類舊的 resetsAt 記錄不會再用到。
            self.sent
                .retain(|(agent, kind, reset, _)| (*agent, *kind) != (reading.agent, reading.kind) || *reset == resets_at);
            for threshold in THRESHOLDS {
                let id = (reading.agent, reading.kind, resets_at, threshold);
                if reading.used_percentage < f64::from(threshold) || self.sent.contains(&id) {
                    continue;
                }
                let mut delivered = false;
                for pairing in &pairings {
                    let body = serde_json::json!({
                        "key": pairing.key,
                        "status": "usage",
                        "agent": reading.agent.name(),
                        "threshold": threshold,
                        "resetsAt": resets_at,
                    });
                    match transport.post_json(&pairing.endpoint, &body.to_string()) {
                        Ok(REVOKED) => pairing.remove_if_unchanged(),
                        // 只有 2xx 才算送達(與外掛一致);4xx/5xx、連不上都留待下次重試。
                        Ok(200..=299) => delivered = true,
                        Ok(_) | Err(_) => {}
                    }
                }
                if delivered {
                    self.sent.insert(id);
                }
            }
        }
    }
}

/// 檢查頻率。
#[derive(Debug, Clone, Copy)]
pub struct UsagePushOptions {
    /// 讀 Claude 用量檔的間隔。
    pub claude_interval: Duration,
    /// 起 `codex app-server` 讀 Codex 用量的最短間隔(成本高)。
    pub codex_interval: Duration,
}

impl Default for UsagePushOptions {
    fn default() -> Self {
        UsagePushOptions {
            claude_interval: Duration::from_secs(30),
            codex_interval: Duration::from_secs(10 * 60),
        }
    }
}

/// 啟動常駐 task:定期讀用量並在跨過門檻時推播。沒有任何配對時兩者都不讀(Codex 才不會白起 app-server)。
pub fn spawn(home: PathBuf, codex_cli: CodexCli, options: UsagePushOptions) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut notifier = UsageNotifier::new();
        let mut last_codex: Option<Instant> = None;
        let mut ticks = tokio::time::interval(options.claude_interval);
        loop {
            ticks.tick().await;
            let paired = {
                let home = home.clone();
                tokio::task::spawn_blocking(move || !pairings(&home.join(".purrdr")).is_empty())
                    .await
                    .unwrap_or(false)
            };
            if !paired {
                continue;
            }
            let mut readings = {
                let home = home.clone();
                tokio::task::spawn_blocking(move || claude_usage_reading(&home)).await.unwrap_or_default()
            };
            if last_codex.is_none_or(|last| last.elapsed() >= options.codex_interval) {
                last_codex = Some(Instant::now());
                if let Ok(usage) = codex_usage::read(&codex_cli).await {
                    readings.extend(readings_from(UsageAgent::Codex, &usage.periods));
                }
            }
            let home = home.clone();
            let moved = std::mem::take(&mut notifier);
            notifier = tokio::task::spawn_blocking(move || {
                let mut notifier = moved;
                notifier.notify(&home, &mut HttpPushTransport::default(), &readings);
                notifier
            })
            .await
            .unwrap_or_default();
        }
    })
}
