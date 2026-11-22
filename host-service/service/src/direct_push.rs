//! 主機服務直接送 blocked／done 推播(raychiutw/purrdr.host#48,0.12.0 起)。
//!
//! 去重靠 lease:主機服務在 `~/.purrdr/host-service/push-sender` 記下它觀察的 herdr socket 與更新時間(Unix 秒),
//! 約每 30 秒更新(跟著觀察者的檢查週期);主機外掛 0.2.0 看到新鮮(120 秒內)且同一個 socket 的 lease 就讓位。
//! 主機服務掛掉或 herdr 訂閱斷掉時 lease 被刪或過期，外掛自動接回。
//!
//! 已安裝的外掛是 0.1.0(沒有讓位邏輯)時主機服務不接手、不寫 lease:偵測方式是 `herdr plugin list --plugin purrdr.push --json`
//! 的 `version`(與代裝外掛用的同一支已允許的 herdr 子指令，沒有新增任何會執行的 herdr 指令)。
//! 版本看不懂、查詢失敗一律當作「不能接手」:寧可維持現況(由外掛送)也不冒重複的風險。
//! 假設(尚未在真的 herdr 上逐字驗證):herdr 注入外掛的 `HERDR_SOCKET_PATH` 與這裡解析的 socket 指向同一個檔案;
//! 兩邊都先把資料夾正規化再比，不一致時外掛照送(最壞情況是重複，不會漏推)。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::push_plugin::PushPlugin;

/// lease 檔在 `~/.purrdr/` 底下的位置。不能用 `push-` 開頭(會被當成某支裝置)。
const LEASE_FILE: &str = "host-service/push-sender";
/// 外掛從這個版本起會讀 lease 讓位。
const FIRST_YIELDING_VERSION: (u64, u64, u64) = (0, 2, 0);

/// herdr 裡 purrdr.push 外掛的狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PluginPresence {
    /// 沒裝。
    Absent,
    /// 已裝，且會讓位(版本 >= 0.2.0)。
    Yielding,
    /// 已裝，但是舊版或版本看不懂:不會讓位。
    Legacy,
}

impl PluginPresence {
    /// 已安裝(不論版本)。
    pub fn installed(self) -> bool {
        self != PluginPresence::Absent
    }

    /// 主機服務可以直接送(沒有外掛，或外掛會讓位)。
    pub fn host_service_may_send(self) -> bool {
        self != PluginPresence::Legacy
    }

    /// 解析 `herdr plugin list --json` 的輸出。JSON 讀不懂時視為沒裝(與先前相同)。
    pub fn from_list(stdout: &str) -> PluginPresence {
        let Ok(list) = serde_json::from_str::<serde_json::Value>(stdout.trim()) else {
            return PluginPresence::Absent;
        };
        let Some(plugin) = list["result"]["plugins"]
            .as_array()
            .and_then(|plugins| plugins.iter().find(|plugin| plugin["plugin_id"] == crate::push_plugin::PLUGIN_ID))
        else {
            return PluginPresence::Absent;
        };
        match plugin["version"].as_str().and_then(parse_version) {
            Some(version) if version >= FIRST_YIELDING_VERSION => PluginPresence::Yielding,
            _ => PluginPresence::Legacy,
        }
    }
}

/// `主.次.修` 三段數字(後面可接 `-` 或 `+` 的附註);其他一律 None。
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

/// 觀察者直接送推播所需:查外掛狀態與維護 lease。
pub(crate) struct DirectDelivery {
    plugin: PushPlugin,
    lease: Lease,
}

impl DirectDelivery {
    pub fn new(plugin: PushPlugin, home: &Path, socket_path: &str) -> Self {
        DirectDelivery {
            plugin,
            lease: Lease::new(home, socket_path),
        }
    }

    /// 重新判斷能不能直接送，並讓 lease 跟著變(能送就寫／更新，不能就刪)。回傳現在是否由主機服務送。
    pub async fn refresh(&self) -> bool {
        let presence = tokio::time::timeout(std::time::Duration::from_secs(10), self.plugin.presence()).await;
        if let Ok(Ok(presence)) = presence
            && presence.host_service_may_send()
            && self.lease.write(SystemTime::now()).is_ok()
        {
            return true;
        }
        self.lease.remove();
        false
    }

    pub fn release(&self) {
        self.lease.remove();
    }

    pub fn lease(&self) -> Lease {
        self.lease.clone()
    }
}

/// `~/.purrdr/host-service/push-sender`。
#[derive(Debug, Clone)]
pub(crate) struct Lease {
    path: PathBuf,
    socket: String,
}

impl Lease {
    pub fn new(home: &Path, socket_path: &str) -> Self {
        Lease {
            path: home.join(".purrdr").join(LEASE_FILE),
            socket: canonical_socket(socket_path),
        }
    }

    /// 先寫暫存檔再改名:外掛讀到的一定是完整的檔案。
    pub fn write(&self, now: SystemTime) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let seconds = now.duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_secs());
        let temporary = self.path.with_extension("tmp");
        let mut file = std::fs::File::create(&temporary)?;
        write!(file, "purrdr-lease 1\nsocket={}\nupdated={seconds}\n", self.socket)?;
        drop(file);
        std::fs::rename(&temporary, &self.path)
    }

    /// 只刪自己寫的(socket 相同):不誤刪另一個主機服務(別的 herdr session)的 lease。
    pub fn remove(&self) {
        let ours = std::fs::read_to_string(&self.path).is_ok_and(|text| text.lines().any(|line| line == format!("socket={}", self.socket)));
        if ours {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// 外掛(`notify.sh`)用同樣的算法:資料夾解析符號連結與 `..`,檔名照舊。Windows 的 socket 是 named pipe 名稱，照原樣
/// (外掛端以大小寫與斜線方向不敏感的方式比)。
fn canonical_socket(path: &str) -> String {
    if cfg!(windows) {
        return path.to_owned();
    }
    let path_buf = Path::new(path);
    let canonical = path_buf
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .and_then(|parent| std::fs::canonicalize(parent).ok())
        .zip(path_buf.file_name())
        .map(|(directory, name)| directory.join(name).to_string_lossy().into_owned());
    canonical.unwrap_or_else(|| path.to_owned())
}
