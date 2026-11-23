//! 沒辦法詢問時的安裝與 `--json`(raychiutw/purrdr.host#84)。詢問條件是純函式，安裝用假系統。

mod install_support;

use install_support::{FQDN, FakeSystem, TestHost, snapshot};

use purrdr_host_service::install::non_interactive::{asks_questions, failure_json, install_without_prompting, success_json};
use purrdr_host_service::install::{InstallError, InstallPlatform, InstallQuestion, Installer};

fn no_environment(_: &str) -> Option<String> {
    None
}

#[test]
fn asks_only_with_a_terminal_and_nothing_that_says_not_to() {
    assert!(asks_questions(false, false, no_environment, true, true));
    // 沒有終端機:stdin 或 stdout 任一不是。
    assert!(!asks_questions(false, false, no_environment, false, true));
    assert!(!asks_questions(false, false, no_environment, true, false));
    // --yes(含 PURRDR_YES=1,呼叫端已併入)與 --json。
    assert!(!asks_questions(true, false, no_environment, true, true));
    assert!(!asks_questions(false, true, no_environment, true, true));
    // CI 環境變數。
    assert!(!asks_questions(false, false, |name| (name == "CI").then(|| "true".to_owned()), true, true));
}

#[test]
fn an_unset_or_false_ci_variable_does_not_stop_the_questions() {
    for value in ["", "0", "false"] {
        let environment = |name: &str| (name == "CI").then(|| value.to_owned());
        assert!(asks_questions(false, false, environment, true, true), "CI={value:?}");
    }
}

#[test]
fn does_not_detect_ai_agents() {
    let environment = |name: &str| (name == "CLAUDECODE").then(|| "1".to_owned());
    assert!(asks_questions(false, false, environment, true, true));
}

#[test]
fn first_install_without_yes_stops_before_changing_anything_and_names_the_public_hostname() {
    let host = TestHost::new(InstallPlatform::Linux);
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    let home_before = snapshot(&host.home.0);

    let error = install_without_prompting(&mut installer, &host.options(47_820), false, |_| {}).unwrap_err();

    assert!(matches!(&error, InstallError::ConsentRequired { notice } if notice.contains(FQDN)), "{error:?}");
    assert_eq!(snapshot(&host.home.0), home_before);
    assert!(installer.system().state.serve.is_empty());
}

#[test]
fn first_install_with_yes_goes_ahead_and_reports_the_notice_it_agreed_to() {
    let host = TestHost::new(InstallPlatform::Linux);
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    let mut asked = Vec::new();

    let installed = install_without_prompting(&mut installer, &host.options(47_820), true, |question| asked.push(question.clone())).unwrap();

    assert_eq!(installed.https_address, format!("https://{FQDN}"));
    assert_eq!(asked, vec![InstallQuestion::CertificateTransparency { host_name: FQDN.to_owned() }]);
}

#[test]
fn reinstall_on_the_same_hostname_needs_no_yes() {
    let host = TestHost::new(InstallPlatform::Linux);
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    installer.install(&host.options(47_820), |_| true).unwrap();

    let installed = install_without_prompting(&mut installer, &host.options(47_820), false, |_| {});

    assert!(installed.is_ok(), "{installed:?}");
}

#[test]
fn settings_summary_lists_every_final_value_with_its_source() {
    let host = TestHost::new(InstallPlatform::Linux);
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    let installed = installer.install(&host.options_on_https_port(47_820, 8443), |_| true).unwrap();

    let summary = installed.settings_summary();

    for expected in [
        "Local port: 47820 (argument)",
        "HTTPS port: 8443 (argument)",
        "herdr session: default (default)",
        "Auto update: on (default)",
        "Add to PATH: yes (default)",
    ] {
        assert!(summary.contains(expected), "缺少 {expected}:\n{summary}");
    }
}

#[test]
fn success_json_has_address_settings_with_sources_warnings_and_pair_command() {
    let host = TestHost::new(InstallPlatform::Linux);
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    let installed = installer.install(&host.options_on_https_port(47_820, 8443), |_| true).unwrap();

    let json = success_json(&installed);

    assert_eq!(json["ok"], true);
    assert_eq!(json["https_address"], format!("https://{FQDN}:8443"));
    assert_eq!(json["settings"]["port"], serde_json::json!({ "value": 47_820, "source": "argument" }));
    assert_eq!(json["settings"]["https_port"], serde_json::json!({ "value": 8443, "source": "argument" }));
    assert_eq!(json["settings"]["herdr_session"], serde_json::json!({ "value": null, "source": "default" }));
    assert_eq!(json["settings"]["auto_update"], serde_json::json!({ "value": true, "source": "default" }));
    assert_eq!(json["settings"]["add_to_path"], serde_json::json!({ "value": true, "source": "default" }));
    assert!(json["warnings"].is_array());
    assert!(json["pair_command"].as_str().unwrap().ends_with(" pair"), "{json}");
}

#[test]
fn failure_json_has_the_error_and_the_rerun_command() {
    let json = failure_json(&InstallError::TailscaleMissing, "purrdr install --json");

    assert_eq!(json["ok"], false);
    assert_eq!(json["error"]["kind"], "tailscale_missing");
    assert!(json["error"]["message"].as_str().unwrap().contains("Tailscale is not installed"), "{json}");
    assert_eq!(json["rerun_command"], "purrdr install --json");
}

#[test]
fn consent_failure_json_says_consent_is_needed() {
    let error = InstallError::ConsentRequired { notice: "notice text".into() };

    let json = failure_json(&error, "purrdr install --json --yes");

    assert_eq!(json["error"]["kind"], "consent_required");
    assert!(json["error"]["message"].as_str().unwrap().contains("notice text"), "{json}");
    assert_eq!(json["rerun_command"], "purrdr install --json --yes");
}
