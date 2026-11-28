//! 方案用量推播(raychiutw/purrdr.host#98):門檻判斷、每個週期每個門檻只送一次、週期重置後可再送、
//! 沒有用量檔或沒有重置時間不送。以假的 transport 與暫存家目錄驗證(不起 codex)。

use std::path::Path;

use purrdr_host_service::protocol::{PlanUsagePeriod, PlanUsagePeriodKind};
use purrdr_host_service::update_push::PushTransport;
use purrdr_host_service::usage_push::{Reading, UsageAgent, UsageNotifier, claude_usage_reading, readings_from};
use serde_json::{Value, json};
use tempfile::TempDir;

const KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";

#[derive(Default)]
struct Recorder {
    bodies: Vec<Value>,
    status: Option<u16>,
}

impl PushTransport for Recorder {
    fn post_json(&mut self, _endpoint: &str, body: &str) -> Result<u16, String> {
        self.bodies.push(serde_json::from_str(body).unwrap());
        Ok(self.status.unwrap_or(204))
    }
}

fn pair(home: &Path) {
    let directory = home.join(".purrdr/push-phone");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("pairing"),
        format!("purrdr-push 1\nendpoint=https://example.test/v1/push\nkey={KEY}\n"),
    )
    .unwrap();
}

fn reading(agent: UsageAgent, kind: PlanUsagePeriodKind, used: f64, resets_at: Option<u64>) -> Reading {
    Reading {
        agent,
        kind,
        used_percentage: used,
        resets_at,
    }
}

fn claude(used: f64, resets: Option<u64>) -> Reading {
    reading(UsageAgent::Claude, PlanUsagePeriodKind::FiveHour, used, resets)
}

fn thresholds(transport: &Recorder) -> Vec<u64> {
    transport.bodies.iter().map(|body| body["threshold"].as_u64().unwrap()).collect()
}

#[test]
fn below_eighty_sends_nothing() {
    let home = TempDir::new().unwrap();
    pair(home.path());
    let (mut notifier, mut transport) = (UsageNotifier::new(), Recorder::default());
    notifier.notify(home.path(), &mut transport, &[claude(79.9, Some(1000))]);
    assert!(transport.bodies.is_empty());
}

#[test]
fn eighty_sends_the_documented_body_without_workspace_pane_or_tab() {
    let home = TempDir::new().unwrap();
    pair(home.path());
    let (mut notifier, mut transport) = (UsageNotifier::new(), Recorder::default());
    notifier.notify(home.path(), &mut transport, &[claude(80.0, Some(1000))]);
    assert_eq!(
        transport.bodies,
        vec![json!({"key": KEY, "status": "usage", "agent": "claude", "threshold": 80, "resetsAt": 1000})]
    );
}

#[test]
fn crossing_both_thresholds_at_once_sends_each() {
    let home = TempDir::new().unwrap();
    pair(home.path());
    let (mut notifier, mut transport) = (UsageNotifier::new(), Recorder::default());
    notifier.notify(
        home.path(),
        &mut transport,
        &[reading(UsageAgent::Codex, PlanUsagePeriodKind::Week, 96.0, Some(2000))],
    );
    assert_eq!(thresholds(&transport), vec![80, 95]);
    assert!(transport.bodies.iter().all(|body| body["agent"] == "codex"));
}

#[test]
fn each_threshold_is_sent_once_per_period() {
    let home = TempDir::new().unwrap();
    pair(home.path());
    let (mut notifier, mut transport) = (UsageNotifier::new(), Recorder::default());
    notifier.notify(home.path(), &mut transport, &[claude(85.0, Some(1000))]);
    notifier.notify(home.path(), &mut transport, &[claude(86.0, Some(1000))]);
    assert_eq!(thresholds(&transport), vec![80]);
    notifier.notify(home.path(), &mut transport, &[claude(96.0, Some(1000))]);
    assert_eq!(thresholds(&transport), vec![80, 95]);
}

#[test]
fn a_new_period_sends_again() {
    let home = TempDir::new().unwrap();
    pair(home.path());
    let (mut notifier, mut transport) = (UsageNotifier::new(), Recorder::default());
    notifier.notify(home.path(), &mut transport, &[claude(85.0, Some(1000))]);
    notifier.notify(home.path(), &mut transport, &[claude(10.0, Some(19000))]);
    notifier.notify(home.path(), &mut transport, &[claude(85.0, Some(19000))]);
    let resets: Vec<u64> = transport.bodies.iter().map(|body| body["resetsAt"].as_u64().unwrap()).collect();
    assert_eq!(resets, vec![1000, 19000]);
}

#[test]
fn no_reset_time_sends_nothing() {
    let home = TempDir::new().unwrap();
    pair(home.path());
    let (mut notifier, mut transport) = (UsageNotifier::new(), Recorder::default());
    notifier.notify(home.path(), &mut transport, &[claude(99.0, None)]);
    assert!(transport.bodies.is_empty());
}

#[test]
fn five_hour_and_week_are_separate_periods() {
    let home = TempDir::new().unwrap();
    pair(home.path());
    let (mut notifier, mut transport) = (UsageNotifier::new(), Recorder::default());
    notifier.notify(
        home.path(),
        &mut transport,
        &[
            claude(81.0, Some(1000)),
            reading(UsageAgent::Claude, PlanUsagePeriodKind::Week, 81.0, Some(9000)),
        ],
    );
    assert_eq!(transport.bodies.len(), 2);
}

#[test]
fn without_pairings_nothing_is_sent_and_it_is_not_remembered() {
    let home = TempDir::new().unwrap();
    let (mut notifier, mut transport) = (UsageNotifier::new(), Recorder::default());
    notifier.notify(home.path(), &mut transport, &[claude(90.0, Some(1000))]);
    assert!(transport.bodies.is_empty());
    pair(home.path());
    notifier.notify(home.path(), &mut transport, &[claude(90.0, Some(1000))]);
    assert_eq!(thresholds(&transport), vec![80]);
}

#[test]
fn a_revoked_pairing_is_removed() {
    let home = TempDir::new().unwrap();
    pair(home.path());
    let (mut notifier, mut transport) = (
        UsageNotifier::new(),
        Recorder {
            status: Some(410),
            ..Recorder::default()
        },
    );
    notifier.notify(home.path(), &mut transport, &[claude(90.0, Some(1000))]);
    assert!(!home.path().join(".purrdr/push-phone").exists());
}

#[test]
fn readings_parse_the_rfc3339_reset_time_and_keep_periods_without_one() {
    let periods = vec![
        PlanUsagePeriod {
            kind: PlanUsagePeriodKind::FiveHour,
            used_percentage: 90.0,
            resets_at: Some("1970-01-01T00:16:40Z".to_owned()),
        },
        PlanUsagePeriod {
            kind: PlanUsagePeriodKind::Week,
            used_percentage: 90.0,
            resets_at: None,
        },
    ];
    let readings = readings_from(UsageAgent::Claude, &periods);
    assert_eq!(readings.len(), 2);
    assert_eq!(readings[0].resets_at, Some(1000));
    assert_eq!(readings[1].resets_at, None);
}

#[test]
fn claude_usage_file_missing_yields_no_readings() {
    let home = TempDir::new().unwrap();
    assert!(claude_usage_reading(home.path()).is_empty());
}

#[test]
fn claude_usage_file_yields_readings_for_a_future_reset() {
    let home = TempDir::new().unwrap();
    let directory = home.path().join(".purrdr/claude-usage");
    std::fs::create_dir_all(&directory).unwrap();
    let resets = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 3600;
    let input = json!({"rate_limits": {"five_hour": {"used_percentage": 91.0, "resets_at": resets}}});
    std::fs::write(directory.join("usage.json"), json!({"captured_at": 1, "status_line_input": input}).to_string()).unwrap();
    let readings = claude_usage_reading(home.path());
    assert_eq!(readings, vec![reading(UsageAgent::Claude, PlanUsagePeriodKind::FiveHour, 91.0, Some(resets))]);
}
