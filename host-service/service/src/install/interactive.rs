//! 互動安裝(raychiutw/purrdr.host#83):安裝前摘要、選單與逐項自訂。畫面維持英文。

use std::io::{BufRead, Write};

use super::settings::{Source, Sourced};
use super::system::HostSystem;
use super::{InstallError, InstallOptions, InstallPlan, InstallQuestion, Installed, Installer, certificate_transparency_notice};

/// 安裝程式的詢問介面。實作只需提供「顯示一段文字」與「顯示提示並讀一行」;
/// 選單、帶預設值的文字／數值與是／否都由預設方法完成，輸入不合法時重問。
pub trait Prompter {
    /// 顯示一段說明文字(結尾自動換行)。
    fn say(&mut self, text: &str);
    /// 顯示提示並讀一行回答(不含換行);輸入已經結束時回 `None`。
    fn read_line(&mut self, prompt: &str) -> Option<String>;

    /// 選單：列出 `items`(從 1 起編號)，空白回答選 `default`(0 起算的索引)，不認得的回答重問;輸入結束回 `None`。
    fn choose(&mut self, items: &[String], default: usize) -> Option<usize> {
        for (index, item) in items.iter().enumerate() {
            self.say(&format!("  {}) {item}", index + 1));
        }
        let prompt = format!("Choose [{}]: ", default + 1);
        loop {
            let answer = self.read_line(&prompt)?;
            let answer = answer.trim();
            if answer.is_empty() {
                return Some(default);
            }
            match answer.parse::<usize>() {
                Ok(number) if (1..=items.len()).contains(&number) => return Some(number - 1),
                _ => self.say(&format!("Enter a number from 1 to {}.", items.len())),
            }
        }
    }

    /// 帶預設值的提問(`label [default]: `):空白回答用預設值，`validate` 不接受就顯示原因並重問;輸入結束回 `None`。
    fn ask_validated<T>(&mut self, label: &str, default: &str, mut validate: impl FnMut(&str) -> Result<T, String>) -> Option<T> {
        let prompt = format!("{label} [{default}]: ");
        loop {
            let answer = self.read_line(&prompt)?;
            let answer = answer.trim();
            match validate(if answer.is_empty() { default } else { answer }) {
                Ok(value) => return Some(value),
                Err(reason) => self.say(&reason),
            }
        }
    }

    /// 是／否(`label (Y/n): `);空白回答用 `default`。
    fn ask_yes_no(&mut self, label: &str, default: bool) -> Option<bool> {
        let prompt = format!("{label} ({}): ", if default { "Y/n" } else { "y/N" });
        loop {
            let answer = self.read_line(&prompt)?;
            match answer.trim().to_ascii_lowercase().as_str() {
                "" => return Some(default),
                "y" | "yes" => return Some(true),
                "n" | "no" => return Some(false),
                _ => self.say("Answer y or n."),
            }
        }
    }
}

/// 真正的終端機：說明印到 stdout，從 stdin 讀回答。
pub struct TerminalPrompter;

impl Prompter for TerminalPrompter {
    fn say(&mut self, text: &str) {
        println!("{text}");
    }

    fn read_line(&mut self, prompt: &str) -> Option<String> {
        print!("{prompt}");
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        match std::io::stdin().lock().read_line(&mut answer) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(answer.trim_end_matches(['\r', '\n']).to_owned()),
        }
    }
}

const INSTALL: usize = 0;
const CUSTOMIZE: usize = 1;

/// 先顯示摘要與選單，再依選擇安裝、逐項自訂或取消(取消時不改任何東西，回 `InstallError::Declined`)。
/// 選「安裝」就同意摘要裡的一切，包括公開主機名稱的提醒與換用其他 HTTPS 埠，不會再問一次。
pub fn install_interactively<S: HostSystem>(
    installer: &mut Installer<S>,
    mut options: InstallOptions,
    prompter: &mut impl Prompter,
) -> Result<Installed, InstallError> {
    let plan = loop {
        let plan = installer.plan(&options)?;
        prompter.say(&summary(&plan));
        let keep = plan.reinstall && options.requested == Default::default();
        let items = [
            if keep {
                "Keep current settings (default)"
            } else {
                "Install with these settings (default)"
            }
            .to_owned(),
            "Customize".to_owned(),
            "Cancel".to_owned(),
        ];
        match prompter.choose(&items, INSTALL) {
            Some(INSTALL) => break plan,
            Some(CUSTOMIZE) => {
                if customize(installer, &mut options, &plan, prompter).is_none() {
                    return Err(InstallError::Declined);
                }
            }
            _ => return Err(InstallError::Declined),
        }
    };
    let https_port = plan.settings.https_port.value;
    installer.install(&options, |question| match question {
        InstallQuestion::CertificateTransparency { .. } => true,
        InstallQuestion::UseAnotherHttpsPort { alternative, .. } if *alternative == https_port => true,
        other => {
            prompter.say(&other.to_string());
            prompter.ask_yes_no("Continue", false).unwrap_or(false)
        }
    })
}

fn chosen<T>(value: T) -> Sourced<T> {
    Sourced { value, source: Source::Prompt }
}

/// 逐項詢問，把和目前不同的值記進 `options`;輸入中途結束回 `None`。
fn customize<S: HostSystem>(installer: &mut Installer<S>, options: &mut InstallOptions, plan: &InstallPlan, prompter: &mut impl Prompter) -> Option<()> {
    let current = &plan.settings;
    prompter.say("Press Enter to keep the value in brackets.");
    let port = prompter.ask_validated("Local port", &current.port.value.to_string(), |text| {
        text.parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| "That is not a port number (1-65535).".to_owned())
    })?;
    let https_port = prompter.ask_validated("HTTPS port", &current.https_port.value.to_string(), |text| {
        let https_port = text
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| "That is not a port number (1-65535).".to_owned())?;
        match installer.https_port_taken_by(https_port, port) {
            Ok(Some(handlers)) => Err(format!("{https_port} is used by {handlers}. Choose another port.")),
            Ok(None) => Ok(https_port),
            Err(error) => Err(error.to_string()),
        }
    })?;
    let session_default = current.herdr_session.value.as_deref().unwrap_or("default");
    let session = prompter.ask_validated("herdr session", session_default, |text| Ok(text.to_owned()))?;
    let auto_update = prompter.ask_yes_no("Auto update", current.auto_update.value)?;
    let add_to_path = prompter.ask_yes_no("Add to PATH", current.add_to_path.value)?;

    let requested = &mut options.requested;
    if port != current.port.value {
        requested.port = Some(chosen(port));
    }
    if https_port != current.https_port.value {
        requested.https_port = Some(chosen(https_port));
    }
    if session != session_default {
        requested.herdr_session = Some(chosen(session));
    }
    if auto_update != current.auto_update.value {
        requested.auto_update = Some(chosen(auto_update));
    }
    if add_to_path != current.add_to_path.value {
        requested.add_to_path = Some(chosen(add_to_path));
    }
    Some(())
}

/// 安裝前摘要：本機埠、HTTPS 埠與被占用的原因、HTTPS 網址、herdr session、自動更新、加入 PATH，需要時加上憑證公開提醒。
pub fn summary(plan: &InstallPlan) -> String {
    let settings = &plan.settings;
    let yes_no = |value: bool| if value { "yes" } else { "no" };
    let mut lines = vec![
        if plan.reinstall {
            "Purrdr host service is already installed. Settings:"
        } else {
            "Purrdr host service will be installed with these settings:"
        }
        .to_owned(),
        format!("  Local port: {}", settings.port.value),
        format!("  HTTPS port: {}", settings.https_port.value),
    ];
    if let Some(conflict) = &plan.https_port_conflict {
        let reasons = conflict
            .skipped
            .iter()
            .map(|skipped| format!("{} is used by {}", skipped.port, skipped.handlers))
            .collect::<Vec<_>>()
            .join("; ");
        let outcome = if conflict.port == settings.https_port.value {
            "choose another port with Customize"
        } else {
            "using the next free port"
        };
        lines.push(format!("    ({reasons}; {outcome})"));
    }
    lines.push(format!("  HTTPS address: {}", plan.https_address));
    lines.push(format!("  herdr session: {}", settings.herdr_session.value.as_deref().unwrap_or("default")));
    lines.push(format!("  Auto update: {}", if settings.auto_update.value { "on" } else { "off" }));
    lines.push(format!("  Add to PATH: {}", yes_no(settings.add_to_path.value)));
    if plan.certificate_notice {
        lines.push(String::new());
        lines.push(certificate_transparency_notice(&plan.host_name));
    }
    lines.push(String::new());
    lines.join("\n")
}
