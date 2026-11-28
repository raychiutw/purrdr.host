//! 執行中的主機服務寫下自己的版本(`running.json`),更新流程以它確認重啟後跑的是新版。

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "running.json";

/// `running.json` 的內容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningService {
    pub version: String,
    pub port: u16,
    pub pid: u32,
    /// 開始聽埠的時間(Unix 毫秒)。
    pub started_at_ms: u64,
}

/// 開始聽 `port` 之後呼叫。
pub fn write(state_directory: &Path, port: u16) -> std::io::Result<()> {
    let marker = RunningService {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        port,
        pid: std::process::id(),
        started_at_ms: unix_millis(SystemTime::now()),
    };
    std::fs::create_dir_all(state_directory)?;
    std::fs::write(state_directory.join(FILE_NAME), serde_json::to_string(&marker).expect("可序列化"))
}

pub fn read(state_directory: &Path) -> Option<RunningService> {
    serde_json::from_str(&std::fs::read_to_string(state_directory.join(FILE_NAME)).ok()?).ok()
}

/// 刪掉 `running.json`(正常關閉、或已回報過消失的上次執行);沒有檔案不算錯。
pub fn remove(state_directory: &Path) {
    let _ = std::fs::remove_file(state_directory.join(FILE_NAME));
}

pub fn unix_millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as u64)
}
