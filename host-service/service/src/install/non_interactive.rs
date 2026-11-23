//! 沒辦法詢問時的安裝與 `--json` 輸出(raychiutw/purrdr.host#84)。
//!
//! 不詢問的條件只有:沒有終端機、`CI` 環境變數、`--yes`／`PURRDR_YES=1`、`--json`。
//! 刻意不偵測 `CLAUDECODE` 或其他特定 AI agent(見 spec #80)。

use serde_json::{Value, json};

use super::settings::Sourced;
use super::system::HostSystem;
use super::{InstallError, InstallOptions, InstallQuestion, Installed, Installer};

/// 要不要用終端機詢問使用者。`assume_yes` 已含 `PURRDR_YES=1`;`--json` 隱含不詢問。
pub fn asks_questions(assume_yes: bool, json: bool, environment: impl Fn(&str) -> Option<String>, stdin_terminal: bool, stdout_terminal: bool) -> bool {
    let in_ci = environment("CI").is_some_and(|value| !matches!(value.trim(), "" | "0" | "false"));
    !assume_yes && !json && !in_ci && stdin_terminal && stdout_terminal
}

/// 不詢問地安裝。有 `assume_yes` 就視為同意所有問題(每個問題先交給 `on_question`，讓呼叫端記錄或顯示);
/// 沒有時，第一次需要同意憑證公開(或主機名稱改變)就停止、什麼都不改，回 `ConsentRequired`。
/// 換用其他 HTTPS 埠同樣需要 `--yes`;沒有時照原本的 `ServePortTaken` 停止，錯誤裡已有改用其他埠的重跑指令。
pub fn install_without_prompting<S: HostSystem>(
    installer: &mut Installer<S>,
    options: &InstallOptions,
    assume_yes: bool,
    mut on_question: impl FnMut(&InstallQuestion),
) -> Result<Installed, InstallError> {
    let mut needs_consent = None;
    let result = installer.install(options, |question| {
        if assume_yes {
            on_question(question);
            return true;
        }
        if matches!(question, InstallQuestion::CertificateTransparency { .. }) {
            needs_consent = Some(question.to_string());
        }
        false
    });
    match (result, needs_consent) {
        (Err(InstallError::Declined), Some(notice)) => Err(InstallError::ConsentRequired { notice }),
        (result, _) => result,
    }
}

fn sourced<T: Into<Value> + Clone>(setting: &Sourced<T>) -> Value {
    json!({ "value": setting.value.clone().into(), "source": setting.source.as_str() })
}

/// `--json` 成功時的輸出:HTTPS 網址、各設定的值與來源、警告、配對指令。
pub fn success_json(installed: &Installed) -> Value {
    let settings = &installed.settings;
    json!({
        "ok": true,
        "https_address": installed.https_address,
        "settings": {
            "port": sourced(&settings.port),
            "https_port": sourced(&settings.https_port),
            "herdr_session": sourced(&settings.herdr_session),
            "auto_update": sourced(&settings.auto_update),
            "add_to_path": sourced(&settings.add_to_path),
        },
        "warnings": installed.warnings,
        "pair_command": installed.pair_command,
    })
}

/// `--json` 失敗時的輸出:錯誤種類與說明，加上重跑指令。
pub fn failure_json(error: &InstallError, rerun: &str) -> Value {
    json!({
        "ok": false,
        "error": { "kind": error.kind(), "message": error.explain(rerun) },
        "rerun_command": rerun,
    })
}
