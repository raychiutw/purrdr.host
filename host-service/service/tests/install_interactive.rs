//! 互動安裝：安裝前摘要、選單與自訂(#83)。詢問介面用假回答，系統用假 tailscale。

mod install_support;

use install_support::{FQDN, FakeSystem, ScriptedPrompter, TestHost, occupy_https_port, snapshot};

use purrdr_host_service::install::interactive::install_interactively;
use purrdr_host_service::install::settings::{RequestedSettings, Source};
use purrdr_host_service::install::{InstallError, InstallOptions, InstallPlatform, Installer};

fn defaults(host: &TestHost) -> InstallOptions {
    InstallOptions {
        requested: RequestedSettings::default(),
        source_directory: host.downloads.0.clone(),
    }
}

fn host_with(platform: InstallPlatform) -> (TestHost, Installer<FakeSystem>) {
    let host = TestHost::new(platform);
    let installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    (host, installer)
}

#[test]
fn enter_installs_with_the_suggested_settings_after_showing_everything_that_will_happen() {
    let (host, mut installer) = host_with(InstallPlatform::Linux);
    let mut prompter = ScriptedPrompter::new(&[""]);

    let installed = install_interactively(&mut installer, defaults(&host), &mut prompter).unwrap();

    let screen = &prompter.transcript;
    for expected in [
        "Local port: 47820",
        "HTTPS port: 443",
        &format!("HTTPS address: https://{FQDN}"),
        "herdr session: default",
        "Auto update: on",
        "Add to PATH: yes",
        "1) Install with these settings (default)",
        "2) Customize",
        "3) Cancel",
    ] {
        assert!(screen.contains(expected), "摘要缺少 {expected:?}:\n{screen}");
    }
    assert_eq!(installed.https_address, format!("https://{FQDN}"));
    assert!(installer.system().state.serve.contains_key(&format!("{FQDN}:443")));
    assert!(prompter.unused().is_empty());
}

#[test]
fn summary_says_why_the_default_https_port_was_skipped_and_which_one_is_used() {
    let (host, mut installer) = host_with(InstallPlatform::MacOs);
    occupy_https_port(installer.system_mut(), 443, "http://127.0.0.1:8765");
    let mut prompter = ScriptedPrompter::new(&[""]);

    let installed = install_interactively(&mut installer, defaults(&host), &mut prompter).unwrap();

    let screen = &prompter.transcript;
    assert!(screen.contains("HTTPS port: 8443"), "{screen}");
    assert!(screen.contains("443 is used by / -> http://127.0.0.1:8765"), "{screen}");
    assert_eq!(installed.https_address, format!("https://{FQDN}:8443"));
    assert!(!screen.contains("Use HTTPS 8443?"), "選 1 就是同意換埠，不再問一次:\n{screen}");
}

#[test]
fn a_suggested_https_port_is_labelled_suggested_in_the_plan_and_the_result() {
    let (host, mut installer) = host_with(InstallPlatform::MacOs);
    occupy_https_port(installer.system_mut(), 443, "http://127.0.0.1:8765");

    let plan = installer.plan(&defaults(&host)).unwrap();
    let installed = install_interactively(&mut installer, defaults(&host), &mut ScriptedPrompter::new(&[""])).unwrap();

    assert_eq!((plan.settings.https_port.value, plan.settings.https_port.source), (8443, Source::Suggested));
    assert_eq!(installed.settings.https_port.source, Source::Suggested);
}

#[test]
fn cancel_changes_nothing() {
    let (host, mut installer) = host_with(InstallPlatform::Linux);
    let system_before = installer.system().state.clone();
    let mut prompter = ScriptedPrompter::new(&["3"]);

    let error = install_interactively(&mut installer, defaults(&host), &mut prompter).unwrap_err();

    assert_eq!(error, InstallError::Declined);
    assert!(snapshot(&host.home.0).is_empty());
    assert_eq!(installer.system().state, system_before);
}

#[test]
fn closed_input_cancels_and_an_unknown_menu_choice_asks_again() {
    let (host, mut installer) = host_with(InstallPlatform::Linux);
    let mut prompter = ScriptedPrompter::new(&["9", "x"]);

    let error = install_interactively(&mut installer, defaults(&host), &mut prompter).unwrap_err();

    assert_eq!(error, InstallError::Declined);
    assert_eq!(prompter.prompts.len(), 3, "{:?}", prompter.prompts);
    assert!(snapshot(&host.home.0).is_empty());
}

#[test]
fn customizing_asks_each_setting_with_its_default_and_blank_keeps_the_default() {
    let (host, mut installer) = host_with(InstallPlatform::Linux);
    // 2 自訂:本機埠、HTTPS 埠(空白)、session、自動更新、PATH;回到選單後 Enter 安裝。
    let mut prompter = ScriptedPrompter::new(&["2", "48000", "", "work", "n", "", ""]);

    let installed = install_interactively(&mut installer, defaults(&host), &mut prompter).unwrap();

    assert_eq!(
        prompter.prompts[1..6],
        [
            "Local port [47820]: ",
            "HTTPS port [443]: ",
            "herdr session [default]: ",
            "Auto update (Y/n): ",
            "Add to PATH (Y/n): "
        ]
    );
    let settings = installed.settings;
    assert_eq!((settings.port.value, settings.port.source), (48_000, Source::Prompt));
    assert_eq!((settings.https_port.value, settings.https_port.source), (443, Source::Default));
    assert_eq!(settings.herdr_session.value.as_deref(), Some("work"));
    assert!(!settings.auto_update.value);
    assert!(settings.add_to_path.value);
    assert!(installer.system().state.serve[&format!("{FQDN}:443")]["/"].ends_with(":48000"));
    // 自訂後回到摘要，新的值出現在第二份摘要裡。
    assert!(prompter.transcript.matches("Local port: 48000").count() == 1, "{}", prompter.transcript);
}

#[test]
fn customizing_rejects_bad_ports_and_taken_https_ports_and_asks_again() {
    let (host, mut installer) = host_with(InstallPlatform::Windows);
    occupy_https_port(installer.system_mut(), 8443, "http://127.0.0.1:3000");
    let mut prompter = ScriptedPrompter::new(&["2", "abc", "70000", "47821", "0", "8443", "10000", "", "", "", ""]);

    let installed = install_interactively(&mut installer, defaults(&host), &mut prompter).unwrap();

    let screen = &prompter.transcript;
    assert!(screen.contains("not a port number"), "{screen}");
    assert!(screen.contains("8443 is used by / -> http://127.0.0.1:3000"), "{screen}");
    assert_eq!(installed.settings.port.value, 47_821);
    assert_eq!(installed.settings.https_port.value, 10_000);
    assert_eq!(installed.https_address, format!("https://{FQDN}:10000"));
    assert!(prompter.unused().is_empty());
}

#[test]
fn installing_again_offers_to_keep_the_current_settings() {
    let (host, mut installer) = host_with(InstallPlatform::Linux);
    install_interactively(&mut installer, defaults(&host), &mut ScriptedPrompter::new(&["2", "48000", "", "", "", "", ""])).unwrap();
    let mut prompter = ScriptedPrompter::new(&[""]);

    let installed = install_interactively(&mut installer, defaults(&host), &mut prompter).unwrap();

    let screen = &prompter.transcript;
    assert!(screen.contains("1) Keep current settings (default)"), "{screen}");
    assert!(!screen.contains("Install with these settings"), "{screen}");
    assert!(screen.contains("Local port: 48000"), "{screen}");
    assert_eq!(installed.settings.port.value, 48_000);
}

#[test]
fn certificate_notice_shows_on_the_first_install_and_when_the_host_name_changes_but_not_otherwise() {
    let (host, mut installer) = host_with(InstallPlatform::Linux);
    let mut first = ScriptedPrompter::new(&[""]);
    install_interactively(&mut installer, defaults(&host), &mut first).unwrap();
    assert!(first.transcript.contains("Certificate Transparency"), "{}", first.transcript);
    assert!(first.transcript.contains(FQDN), "{}", first.transcript);

    let mut again = ScriptedPrompter::new(&[""]);
    install_interactively(&mut installer, defaults(&host), &mut again).unwrap();
    assert!(!again.transcript.contains("Certificate Transparency"), "{}", again.transcript);

    // 上次安裝時的主機名稱不同(例如這台機器改過名)。
    let record_file = host.service_directory().join("install.json");
    let mut record: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&record_file).unwrap()).unwrap();
    record["https_address"] = "https://old-name.tail1234.ts.net".into();
    std::fs::write(&record_file, record.to_string()).unwrap();
    let mut renamed = ScriptedPrompter::new(&[""]);
    install_interactively(&mut installer, defaults(&host), &mut renamed).unwrap();
    assert!(renamed.transcript.contains("Certificate Transparency"), "{}", renamed.transcript);
}
