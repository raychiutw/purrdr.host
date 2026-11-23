//! 沒辦法詢問時的安裝與 `--json` 輸出(raychiutw/purrdr.host#84)。
//!
//! 不詢問的條件只有:沒有終端機、`CI` 環境變數、`--yes`／`PURRDR_YES=1`、`--json`。
//! 刻意不偵測 `CLAUDECODE` 或其他特定 AI agent(見 spec #80)。

use serde_json::{Value, json};

use super::settings::{InstallSettings, Sourced};
use super::system::HostSystem;
use super::{InstallError, InstallOptions, InstallQuestion, Installed, Installer};

/// 要不要用終端機詢問使用者。`assume_yes` 已含 `PURRDR_YES=1`;`--json` 隱含不詢問。
pub fn asks_questions(assume_yes: bool, json: bool, environment: impl Fn(&str) -> Option<String>, stdin_terminal: bool, stdout_terminal: bool) -> bool {
    let in_ci = environment("CI").is_some_and(|value| !matches!(value.trim(), "" | "0" | "false"));
    !assume_yes && !json && !in_ci && stdin_terminal && stdout_terminal
}

/// 不詢問地安裝。有 `assume_yes` 就視為同意所有問題(每個問題先交給 `on_question`，讓呼叫端記錄或顯示);
/// 沒有時，第一次需要同意憑證公開(或主機名稱改變)就停止、什麼都不改，回 `ConsentRequired`。
/// 預設的 HTTPS 埠被占用時不需要 `--yes`，自動改用建議值並在警告說明;使用者指定的埠被占用，或候選都被占用才報錯。
pub fn install_without_prompting<S: HostSystem>(
    installer: &mut Installer<S>,
    options: &InstallOptions,
    assume_yes: bool,
    mut on_question: impl FnMut(&InstallQuestion),
) -> Result<Installed, InstallError> {
    let mut needs_consent = None;
    let mut port_changes = Vec::new();
    let result = installer.install(options, |question| match question {
        // 預設的 HTTPS 埠被占用:不詢問，直接用建議值(只有使用者明確指定的埠被占用才報錯，那在 `install` 裡處理);
        // 原因記進警告。
        InstallQuestion::UseAnotherHttpsPort { alternative, skipped, .. } => {
            let reasons = skipped
                .iter()
                .map(|skipped| format!("{} is used by {}", skipped.port, skipped.handlers))
                .collect::<Vec<_>>()
                .join("; ");
            port_changes.push(format!("HTTPS {reasons}; using HTTPS {alternative} instead"));
            true
        }
        InstallQuestion::CertificateTransparency { .. } if assume_yes => {
            on_question(question);
            true
        }
        InstallQuestion::CertificateTransparency { .. } => {
            needs_consent = Some(question.to_string());
            false
        }
    });
    match (result, needs_consent) {
        (Err(InstallError::Declined), Some(notice)) => Err(InstallError::ConsentRequired { notice }),
        (Ok(mut installed), _) => {
            installed.warnings.splice(0..0, port_changes);
            Ok(installed)
        }
        (result, _) => result,
    }
}

fn sourced<T: Into<Value> + Clone>(setting: &Sourced<T>) -> Value {
    json!({ "value": setting.value.clone().into(), "source": setting.source.as_str() })
}

fn settings_json(settings: &InstallSettings) -> Value {
    json!({
        "port": sourced(&settings.port),
        "https_port": sourced(&settings.https_port),
        "herdr_session": sourced(&settings.herdr_session),
        "auto_update": sourced(&settings.auto_update),
        "add_to_path": sourced(&settings.add_to_path),
    })
}

/// `--json` 成功時的輸出:HTTPS 網址、各設定的值與來源、警告、配對指令。
pub fn success_json(installed: &Installed) -> Value {
    json!({
        "ok": true,
        "https_address": installed.https_address,
        "settings": settings_json(&installed.settings),
        "warnings": installed.warnings,
        "pair_command": installed.pair_command,
    })
}

/// `--json` 失敗時的輸出:錯誤種類與說明，加上重跑指令;已解析到設定時(`settings`)也帶上各項的值與來源。
pub fn failure_json(error: &InstallError, rerun: &str, settings: Option<&InstallSettings>) -> Value {
    let mut output = json!({
        "ok": false,
        "error": { "kind": error.kind(), "message": error.explain(rerun) },
        "rerun_command": rerun,
    });
    if let Some(settings) = settings {
        output["settings"] = settings_json(settings);
    }
    output
}
