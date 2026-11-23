//! 安裝設定的決定順序與來源(raychiutw/purrdr.host#81)。
//!
//! 每個設定依「命令列參數 > `PURRDR_*` 環境變數 > 上次安裝的紀錄 > 預設值」決定，並記下值的來源，
//! 供安裝前摘要、非互動輸出與 `--json` 使用。

use super::InstallCommand;

/// 設定值的來源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// 命令列參數。
    Argument,
    /// `PURRDR_*` 環境變數。
    Environment,
    /// 上次安裝的紀錄。
    Previous,
    /// 預設值。
    Default,
    /// 互動安裝時使用者輸入的值。
    Prompt,
    /// 安裝程式在預設值被占用時自動改用的建議值(警告會說明被跳過的埠)。
    Suggested,
}

impl Source {
    /// 給人與機器讀的名稱(`--json` 也用這個)。
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Argument => "argument",
            Source::Environment => "environment",
            Source::Previous => "previous",
            Source::Default => "default",
            Source::Prompt => "prompt",
            Source::Suggested => "suggested",
        }
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 一個帶來源的設定值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sourced<T> {
    pub value: T,
    pub source: Source,
}

impl<T> Sourced<T> {
    /// 使用者(參數、環境變數或互動輸入)明確指定的值，不是沿用、預設或自動建議。
    pub fn is_explicit(&self) -> bool {
        matches!(self.source, Source::Argument | Source::Environment | Source::Prompt)
    }
}

/// 使用者透過參數或環境變數明確指定的設定;沒指定的是 `None`，由上次安裝的紀錄或預設值補上。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestedSettings {
    pub port: Option<Sourced<u16>>,
    pub https_port: Option<Sourced<u16>>,
    pub herdr_session: Option<Sourced<String>>,
    pub auto_update: Option<Sourced<bool>>,
    pub add_to_path: Option<Sourced<bool>>,
}

/// 這次安裝最後採用的設定與各自的來源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallSettings {
    pub port: Sourced<u16>,
    pub https_port: Sourced<u16>,
    pub herdr_session: Sourced<Option<String>>,
    pub auto_update: Sourced<bool>,
    pub add_to_path: Sourced<bool>,
}

impl InstallSettings {
    /// 每一項的值與來源,一項一行(沒辦法詢問時印出，讓使用者知道採用了什麼)。
    pub fn summary(&self) -> String {
        [
            "Settings:".to_owned(),
            format!("  Local port: {} ({})", self.port.value, self.port.source),
            format!("  HTTPS port: {} ({})", self.https_port.value, self.https_port.source),
            format!("  herdr session: {} ({})", self.herdr_session.value.as_deref().unwrap_or("default"), self.herdr_session.source),
            format!("  Auto update: {} ({})", if self.auto_update.value { "on" } else { "off" }, self.auto_update.source),
            format!("  Add to PATH: {} ({})", if self.add_to_path.value { "yes" } else { "no" }, self.add_to_path.source),
        ]
        .join("\n")
    }
}

/// 環境變數的值不合法。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsError {
    pub variable: &'static str,
    pub value: String,
    pub expected: &'static str,
}

impl SettingsError {
    /// 錯誤說明，附上可重跑的指令。
    pub fn explain(&self, rerun: &str) -> String {
        format!(
            "invalid {}={}: expected {}.\nFix or unset the variable, or pass the option instead, then run:\n  {rerun}",
            self.variable, self.value, self.expected
        )
    }
}

impl SettingsError {
    /// 重跑指令:變數還設著的話原樣重跑只會再失敗，所以先清掉它(POSIX 用 `unset`,Windows PowerShell 用 `Remove-Item Env:`)。
    pub fn rerun_without_variable(&self, rerun: &str, platform: super::InstallPlatform) -> String {
        if platform == super::InstallPlatform::Windows {
            format!("Remove-Item Env:{}; {rerun}", self.variable)
        } else {
            format!("unset {}; {rerun}", self.variable)
        }
    }
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid {}={}: expected {}", self.variable, self.value, self.expected)
    }
}

impl std::error::Error for SettingsError {}

impl RequestedSettings {
    /// 參數優先，其次環境變數;`command` 不是 `install` 時什麼都沒指定。
    pub fn resolve(command: &InstallCommand, environment: impl Fn(&str) -> Option<String>) -> Result<Self, SettingsError> {
        let InstallCommand::Install {
            port,
            herdr_session,
            https_port,
            auto_update,
            add_to_path,
            ..
        } = command
        else {
            return Ok(Self::default());
        };
        // 參數優先;沒有參數才讀環境變數(空字串視為沒設)。
        fn decide<T: Clone>(
            argument: &Option<T>,
            variable: &'static str,
            expected: &'static str,
            environment: &impl Fn(&str) -> Option<String>,
            parse: impl Fn(&str) -> Option<T>,
        ) -> Result<Option<Sourced<T>>, SettingsError> {
            if let Some(value) = argument {
                return Ok(Some(Sourced {
                    value: value.clone(),
                    source: Source::Argument,
                }));
            }
            let Some(text) = environment(variable).filter(|text| !text.trim().is_empty()) else {
                return Ok(None);
            };
            match parse(text.trim()) {
                Some(value) => Ok(Some(Sourced {
                    value,
                    source: Source::Environment,
                })),
                None => Err(SettingsError {
                    variable,
                    value: text,
                    expected,
                }),
            }
        }
        // 0 不是可用的埠(本機埠與 HTTPS 埠一樣)。
        let port_number = |text: &str| text.parse::<u16>().ok().filter(|port| *port != 0);
        Ok(RequestedSettings {
            port: decide(port, "PURRDR_PORT", "a port number (1-65535)", &environment, port_number)?,
            https_port: decide(https_port, "PURRDR_HTTPS_PORT", "a port number (1-65535)", &environment, port_number)?,
            herdr_session: decide(herdr_session, "PURRDR_SESSION", "a herdr session name", &environment, |text| {
                Some(text.to_owned())
            })?,
            auto_update: decide(auto_update, "PURRDR_AUTO_UPDATE", "0 or 1", &environment, flag)?,
            add_to_path: decide(add_to_path, "PURRDR_ADD_TO_PATH", "0 or 1", &environment, flag)?,
        })
    }
}

fn flag(text: &str) -> Option<bool> {
    match text {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

/// 上次安裝的紀錄中與設定有關的部分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviousSettings {
    pub port: u16,
    pub https_port: u16,
    pub herdr_session: Option<String>,
    pub auto_update: bool,
    pub add_to_path: bool,
}

/// 明確指定的值優先，其次上次安裝的紀錄，最後預設值。
fn pick<T: Clone>(requested: Option<&Sourced<T>>, previous: Option<T>, default: T) -> Sourced<T> {
    if let Some(requested) = requested {
        return requested.clone();
    }
    match previous {
        Some(value) => Sourced {
            value,
            source: Source::Previous,
        },
        None => Sourced {
            value: default,
            source: Source::Default,
        },
    }
}

impl InstallSettings {
    /// 命令列參數 > 環境變數(都在 `requested`) > 上次安裝的紀錄 > 預設值。
    pub fn from_options(requested: &RequestedSettings, previous: Option<PreviousSettings>) -> Self {
        let session = requested.herdr_session.as_ref().map(|session| Sourced {
            value: Some(session.value.clone()),
            source: session.source,
        });
        InstallSettings {
            port: pick(requested.port.as_ref(), previous.as_ref().map(|previous| previous.port), crate::DEFAULT_PORT),
            https_port: pick(
                requested.https_port.as_ref(),
                previous.as_ref().map(|previous| previous.https_port),
                super::tailscale::DEFAULT_HTTPS_PORT,
            ),
            herdr_session: pick(session.as_ref(), previous.as_ref().map(|previous| previous.herdr_session.clone()), None),
            auto_update: pick(requested.auto_update.as_ref(), previous.as_ref().map(|previous| previous.auto_update), true),
            add_to_path: pick(requested.add_to_path.as_ref(), previous.as_ref().map(|previous| previous.add_to_path), true),
        }
    }
}

/// `--yes` 或 `PURRDR_YES=1`。
pub fn assume_yes(command: &InstallCommand, environment: impl Fn(&str) -> Option<String>) -> Result<bool, SettingsError> {
    if matches!(command, InstallCommand::Install { assume_yes: true, .. }) {
        return Ok(true);
    }
    match environment("PURRDR_YES").filter(|text| !text.trim().is_empty()) {
        None => Ok(false),
        Some(text) => flag(text.trim()).ok_or(SettingsError {
            variable: "PURRDR_YES",
            value: text,
            expected: "0 or 1",
        }),
    }
}
