//! 安裝設定的決定順序：命令列參數 > `PURRDR_*` 環境變數 > 上次安裝的紀錄 > 預設值(#81)。

mod install_support;

use install_support::{FQDN, FakeSystem, TestHost, occupy_https_port};

use purrdr_host_service::install::settings::{RequestedSettings, Source, assume_yes};
use purrdr_host_service::install::{InstallCommand, InstallOptions, InstallPlatform, Installer};

fn command(text: &str) -> InstallCommand {
    let arguments: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
    InstallCommand::parse(&arguments).unwrap().unwrap()
}

fn environment<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |name| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_owned())
}

fn options(host: &TestHost, command_text: &str, pairs: &[(&str, &str)]) -> InstallOptions {
    InstallOptions {
        requested: RequestedSettings::resolve(&command(command_text), environment(pairs)).unwrap(),
        source_directory: host.downloads.0.clone(),
    }
}

#[test]
fn environment_variables_fill_in_what_the_arguments_leave_out() {
    let pairs = [
        ("PURRDR_PORT", "47999"),
        ("PURRDR_HTTPS_PORT", "9443"),
        ("PURRDR_SESSION", "work"),
        ("PURRDR_AUTO_UPDATE", "0"),
        ("PURRDR_ADD_TO_PATH", "0"),
    ];
    let requested = RequestedSettings::resolve(&command("install"), environment(&pairs)).unwrap();

    assert_eq!(
        (requested.port.as_ref().unwrap().value, requested.port.as_ref().unwrap().source),
        (47_999, Source::Environment)
    );
    assert_eq!(requested.https_port.as_ref().unwrap().value, 9443);
    assert_eq!(requested.herdr_session.as_ref().unwrap().value, "work");
    assert!(!requested.auto_update.as_ref().unwrap().value);
    assert!(!requested.add_to_path.as_ref().unwrap().value);
}

#[test]
fn arguments_win_over_environment_variables() {
    let pairs = [("PURRDR_PORT", "47999"), ("PURRDR_AUTO_UPDATE", "0"), ("PURRDR_ADD_TO_PATH", "0")];
    let requested = RequestedSettings::resolve(&command("install --port 48000 --auto-update --path"), environment(&pairs)).unwrap();

    let port = requested.port.unwrap();
    assert_eq!((port.value, port.source), (48_000, Source::Argument));
    assert!(requested.auto_update.unwrap().value);
    assert!(requested.add_to_path.unwrap().value);
}

#[test]
fn empty_environment_variables_count_as_unset() {
    let requested = RequestedSettings::resolve(&command("install"), environment(&[("PURRDR_PORT", ""), ("PURRDR_SESSION", " ")])).unwrap();

    assert_eq!(requested, RequestedSettings::default());
}

#[test]
fn invalid_environment_values_name_the_variable_and_give_the_rerun_command() {
    for (name, value) in [
        ("PURRDR_PORT", "abc"),
        ("PURRDR_PORT", "0"),
        ("PURRDR_HTTPS_PORT", "0"),
        ("PURRDR_AUTO_UPDATE", "yes"),
        ("PURRDR_ADD_TO_PATH", "2"),
    ] {
        let pairs = [(name, value)];
        let error = RequestedSettings::resolve(&command("install"), environment(&pairs)).unwrap_err();
        let message = error.explain("sh install.sh --");

        assert!(message.contains(name) && message.contains(value), "{message}");
        assert!(message.contains("sh install.sh --"), "{message}");
    }
}

/// 變數還設著，原樣重跑只會再失敗;重跑指令要先清掉那個變數(依平台:POSIX `unset`、PowerShell `Remove-Item Env:`)。
#[test]
fn the_rerun_command_for_a_bad_variable_clears_it_first() {
    let error = RequestedSettings::resolve(&command("install"), environment(&[("PURRDR_PORT", "abc")])).unwrap_err();

    assert_eq!(
        error.rerun_without_variable("purrdr install --yes", InstallPlatform::Linux),
        "unset PURRDR_PORT; purrdr install --yes"
    );
    assert_eq!(
        error.rerun_without_variable("purrdr install --yes", InstallPlatform::MacOs),
        "unset PURRDR_PORT; purrdr install --yes"
    );
    assert_eq!(
        error.rerun_without_variable("purrdr install --yes", InstallPlatform::Windows),
        "Remove-Item Env:PURRDR_PORT; purrdr install --yes"
    );
}

#[test]
fn purrdr_yes_is_the_same_as_the_yes_option() {
    assert!(assume_yes(&command("install"), environment(&[("PURRDR_YES", "1")])).unwrap());
    assert!(assume_yes(&command("install --yes"), environment(&[])).unwrap());
    assert!(!assume_yes(&command("install"), environment(&[("PURRDR_YES", "0")])).unwrap());
    assert!(!assume_yes(&command("install"), environment(&[])).unwrap());
    assert!(assume_yes(&command("install"), environment(&[("PURRDR_YES", "true")])).is_err());
}

#[test]
fn a_first_install_uses_defaults_and_reports_them_as_such() {
    let host = TestHost::new(InstallPlatform::Linux);
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));

    let installed = installer.install(&options(&host, "install", &[]), |_| true).unwrap();

    let settings = installed.settings;
    assert_eq!((settings.port.value, settings.port.source), (47_820, Source::Default));
    assert_eq!((settings.https_port.value, settings.https_port.source), (443, Source::Default));
    assert_eq!((settings.herdr_session.value, settings.herdr_session.source), (None, Source::Default));
    assert_eq!((settings.auto_update.value, settings.auto_update.source), (true, Source::Default));
    assert_eq!((settings.add_to_path.value, settings.add_to_path.source), (true, Source::Default));
}

#[test]
fn a_reinstall_without_options_keeps_the_previous_port_and_session() {
    let host = TestHost::new(InstallPlatform::Linux);
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    installer
        .install(&options(&host, "install --port 47999 --session work", &[]), |_| true)
        .unwrap();

    let installed = installer.install(&options(&host, "install", &[]), |_| true).unwrap();

    let settings = installed.settings;
    assert_eq!((settings.port.value, settings.port.source), (47_999, Source::Previous));
    assert_eq!(
        (settings.herdr_session.value, settings.herdr_session.source),
        (Some("work".to_owned()), Source::Previous)
    );
    let record: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(host.service_directory().join("install.json")).unwrap()).unwrap();
    assert_eq!(record["port"], 47_999);
    assert_eq!(record["herdr_session"], "work");
    assert_eq!(installer.system().state.serve[&format!("{FQDN}:443")]["/"], "http://127.0.0.1:47999");
}

#[test]
fn an_explicit_port_or_session_replaces_the_previous_one() {
    let host = TestHost::new(InstallPlatform::Linux);
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    installer
        .install(&options(&host, "install --port 47999 --session work", &[]), |_| true)
        .unwrap();

    let installed = installer
        .install(&options(&host, "install", &[("PURRDR_PORT", "48000"), ("PURRDR_SESSION", "play")]), |_| true)
        .unwrap();

    let settings = installed.settings;
    assert_eq!((settings.port.value, settings.port.source), (48_000, Source::Environment));
    assert_eq!(settings.herdr_session.value.as_deref(), Some("play"));
}

#[test]
fn an_https_port_from_the_environment_is_never_swapped_for_another_one() {
    let host = TestHost::new(InstallPlatform::Linux);
    let mut system = FakeSystem::ready(&host.home.0);
    occupy_https_port(&mut system, 9443, "http://127.0.0.1:3000");
    let mut installer = Installer::new(host.environment(), system);
    let mut asked = false;

    let result = installer.install(&options(&host, "install", &[("PURRDR_HTTPS_PORT", "9443")]), |_| {
        asked = true;
        true
    });

    assert!(result.is_err());
    assert!(!asked, "指定的埠被占用時不該詢問改用別的埠");
}

#[test]
fn auto_update_and_path_can_be_turned_back_on() {
    let host = TestHost::new(InstallPlatform::Linux);
    let mut installer = Installer::new(host.environment(), FakeSystem::ready(&host.home.0));
    installer.install(&options(&host, "install --no-auto-update --no-path", &[]), |_| true).unwrap();
    let kept = installer.install(&options(&host, "install", &[]), |_| true).unwrap().settings;

    let back_on = installer
        .install(&options(&host, "install --auto-update --path", &[]), |_| true)
        .unwrap()
        .settings;

    assert_eq!((kept.auto_update.value, kept.auto_update.source), (false, Source::Previous));
    assert_eq!((kept.add_to_path.value, kept.add_to_path.source), (false, Source::Previous));
    assert_eq!((back_on.auto_update.value, back_on.auto_update.source), (true, Source::Argument));
    assert!(back_on.add_to_path.value);
    let record: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(host.service_directory().join("install.json")).unwrap()).unwrap();
    assert_eq!(record["auto_update"], true);
    assert_eq!(record["add_to_path"], true);
}
