//! 上傳圖片與檔案(raychiutw/purrdr.host#16、#41):分塊寫到系統暫存目錄底下的 Purrdr 上傳資料夾,檔名由主機服務產生。
//!
//! 上傳檔只給 Agent 讀一次，所以放在系統暫存目錄(macOS／Linux 的 `TMPDIR`、Windows 的 `TEMP`),不放進家目錄的
//! 長期資料夾。Unix 的暫存目錄(例如 Linux 的 `/tmp`)可能多人共用，所以資料夾名稱帶 uid、權限 0700,
//! 而且已存在的資料夾必須是自己的真資料夾(不是 symlink、擁有者是自己、別人進不去)才使用。
//! 舊版寫在家目錄 `.purrdr/uploads/`:不再寫入，只照同樣的 7 天規則清理，清空後移除。

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::task::JoinHandle;

use crate::protocol::{MAXIMUM_FILE_CHUNK, MAXIMUM_FILE_SIZE, UPLOAD_FOLDER};
use crate::service_log::ServiceLog;

/// 舊版家目錄底下的上傳資料夾。
pub const LEGACY_UPLOAD_FOLDER: [&str; 2] = [".purrdr", "uploads"];

/// 舊版上傳資料夾的完整路徑:`<家目錄>/.purrdr/uploads`。
pub fn legacy_upload_path(home: &Path) -> PathBuf {
    LEGACY_UPLOAD_FOLDER.iter().fold(home.to_owned(), |path, segment| path.join(segment))
}

/// 上傳檔保留多久(與 SSH 上傳的 `find -mmin +10080` 相同)。
pub const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// 定期清理的間隔。
pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// 啟動時與之後每 `CLEANUP_INTERVAL` 清掉超過 7 天的上傳檔;`clock` 提供現在時間(測試注入)。
pub fn spawn_cleanup(directories: Vec<UploadDirectory>, log: ServiceLog, clock: impl Fn() -> SystemTime + Send + 'static) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            // 資料夾只有少量檔案，直接在這裡做。
            for directory in &directories {
                if let Err(error) = directory.remove_expired(clock()) {
                    log.record(&format!("cannot clean uploads {}: {error}", directory.path.display()));
                }
            }
            tokio::time::sleep(CLEANUP_INTERVAL).await;
        }
    })
}

/// 一個上傳資料夾:暫存目錄底下的新資料夾，或舊版的家目錄資料夾。
#[derive(Debug, Clone)]
pub struct UploadDirectory {
    path: PathBuf,
    /// 目前使用者的 uid(Unix);拿不到時不能安全使用資料夾。
    #[cfg(unix)]
    owner: Option<u32>,
    /// 舊資料夾清空後移除;新資料夾留著(上傳中的連線還要用)。
    remove_when_empty: bool,
}

impl UploadDirectory {
    /// 暫存目錄底下的上傳資料夾。Unix 以自己建檔的擁有者取得 uid(不需要 unsafe),名稱帶 uid。
    pub fn under_temporary_directory(temporary_directory: &Path) -> Self {
        #[cfg(unix)]
        {
            let owner = current_user_id(temporary_directory).ok();
            let name = match owner {
                Some(uid) => format!("{UPLOAD_FOLDER}-{uid}"),
                None => format!("{UPLOAD_FOLDER}-unknown"),
            };
            UploadDirectory {
                path: temporary_directory.join(name),
                owner,
                remove_when_empty: false,
            }
        }
        #[cfg(not(unix))]
        UploadDirectory {
            path: temporary_directory.join(UPLOAD_FOLDER),
            remove_when_empty: false,
        }
    }

    /// 舊版家目錄的 `.purrdr/uploads/`(只清理、不寫入)。
    pub fn legacy_under_home(home: &Path) -> Self {
        UploadDirectory {
            path: legacy_upload_path(home),
            #[cfg(unix)]
            owner: None,
            remove_when_empty: true,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 刪掉修改時間比 `now` 早超過 7 天的檔案;舊資料夾清空後移除。
    pub fn remove_expired(&self, now: SystemTime) -> io::Result<()> {
        // 暫存目錄可能多人共用:資料夾不是自己的私人資料夾(例如別人換成 symlink)時不碰它，以免跟著走去刪別處的檔案。
        #[cfg(unix)]
        if !self.remove_when_empty {
            let owner = self
                .owner
                .ok_or_else(|| io::Error::other("cannot tell which user owns the temporary directory"))?;
            match verify_private_folder(&self.path, owner) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                other => other?,
            }
        }
        let entries = match std::fs::read_dir(&self.path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let metadata = entry.metadata()?;
            let expired = metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > RETENTION);
            if metadata.is_file() && expired {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        if self.remove_when_empty {
            // 還有檔案時 remove_dir 會失敗，正好不移除。
            let _ = std::fs::remove_dir(&self.path);
        }
        Ok(())
    }

    /// 建立上傳資料夾，或確認已存在的是自己的私人資料夾。
    fn prepare_directory(&self) -> io::Result<()> {
        #[cfg(unix)]
        {
            let owner = self
                .owner
                .ok_or_else(|| io::Error::other("cannot tell which user owns the temporary directory"))?;
            use std::os::unix::fs::DirBuilderExt;
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            match std::fs::DirBuilder::new().mode(0o700).create(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => verify_private_folder(&self.path, owner),
                Err(error) => Err(error),
            }
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(&self.path)
    }

    /// 開始一次上傳：在上傳資料夾建隱藏的暫存檔。
    pub(crate) fn begin(&self, size: u64, extension: Option<&str>) -> Result<PendingUpload, UploadFailure> {
        if size > MAXIMUM_FILE_SIZE {
            return Err(UploadFailure::TooLarge(format!("upload is larger than {MAXIMUM_FILE_SIZE} bytes")));
        }
        let extension = match extension {
            Some(extension) if is_valid_extension(extension) => Some(extension.to_ascii_lowercase()),
            Some(_) => return Err(UploadFailure::Invalid("extension must be 1-16 ASCII letters or digits")),
            None => None,
        };
        self.prepare_directory().map_err(UploadFailure::Storage)?;
        let temporary = self.path.join(format!(".upload.{}", random_base36(16).map_err(UploadFailure::Storage)?));
        let file = create_private_file(&temporary).map_err(UploadFailure::Storage)?;
        Ok(PendingUpload {
            directory: self.path.clone(),
            temporary,
            file: Some(file),
            extension,
            size,
            received: 0,
        })
    }
}

/// 上傳失敗的原因。
#[derive(Debug)]
pub(crate) enum UploadFailure {
    /// 超過 `x-maximum-file-size` 或 `x-maximum-file-chunk`。
    TooLarge(String),
    /// offset 不連續或超過宣告的大小。
    Invalid(&'static str),
    Storage(io::Error),
}

impl UploadFailure {
    /// 稽核紀錄的 result。
    pub fn audit_result(&self) -> &'static str {
        match self {
            UploadFailure::TooLarge(_) => "too_large",
            UploadFailure::Invalid(_) => "invalid",
            UploadFailure::Storage(_) => "failed",
        }
    }
}

/// 一個分塊寫入後的狀態。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ChunkOutcome {
    Partial { received: u64 },
    Stored { received: u64, path: PathBuf },
}

/// 進行中的上傳;drop 時還沒存成正式檔就刪掉暫存檔。
#[derive(Debug)]
pub(crate) struct PendingUpload {
    directory: PathBuf,
    temporary: PathBuf,
    file: Option<File>,
    extension: Option<String>,
    size: u64,
    received: u64,
}

impl PendingUpload {
    /// 宣告的總大小。
    pub fn size(&self) -> u64 {
        self.size
    }

    /// 依序寫入一塊;收滿時換成主機產生的正式檔名。
    pub fn append(&mut self, offset: u64, bytes: &[u8]) -> Result<ChunkOutcome, UploadFailure> {
        if bytes.len() as u64 > MAXIMUM_FILE_CHUNK {
            return Err(UploadFailure::TooLarge(format!("chunk is larger than {MAXIMUM_FILE_CHUNK} bytes")));
        }
        if offset != self.received {
            return Err(UploadFailure::Invalid("offset does not match the bytes received so far"));
        }
        if self.received + bytes.len() as u64 > self.size {
            return Err(UploadFailure::Invalid("chunk goes past the declared size"));
        }
        let file = self.file.as_mut().expect("上傳完成後不再寫入");
        file.write_all(bytes).map_err(UploadFailure::Storage)?;
        self.received += bytes.len() as u64;
        if self.received < self.size {
            return Ok(ChunkOutcome::Partial { received: self.received });
        }
        let file = self.file.take().expect("剛寫入的暫存檔");
        file.sync_all().map_err(UploadFailure::Storage)?;
        drop(file);
        let mut name = stored_name(SystemTime::now()).map_err(UploadFailure::Storage)?;
        if let Some(extension) = &self.extension {
            name = format!("{name}.{extension}");
        }
        let path = self.directory.join(name);
        std::fs::rename(&self.temporary, &path).map_err(UploadFailure::Storage)?;
        Ok(ChunkOutcome::Stored { received: self.received, path })
    }
}

impl Drop for PendingUpload {
    fn drop(&mut self) {
        // 先關檔:Windows 不能刪開著的檔案。
        if let Some(file) = self.file.take() {
            drop(file);
            let _ = std::fs::remove_file(&self.temporary);
        }
    }
}

/// `<時間(毫秒)base36>-<8 個 base36 亂數>`,與 app 的 SSH 上傳檔名同格式。
fn stored_name(now: SystemTime) -> io::Result<String> {
    let millis = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    Ok(format!("{}-{}", to_base36(millis), random_base36(8)?))
}

fn is_valid_extension(extension: &str) -> bool {
    (1..=16).contains(&extension.len()) && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

const BASE36_DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

fn to_base36(mut value: u128) -> String {
    let mut digits = Vec::new();
    loop {
        digits.push(BASE36_DIGITS[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    digits.reverse();
    String::from_utf8(digits).expect("base36 是 ASCII")
}

fn random_base36(length: usize) -> io::Result<String> {
    let mut bytes = vec![0u8; length];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(bytes.iter().map(|byte| BASE36_DIGITS[usize::from(*byte) % 36] as char).collect())
}

/// 暫存目錄裡已存在的上傳資料夾必須是自己的真資料夾:不跟隨 symlink、擁有者是自己、別人進不去(不替別人改權限)。
#[cfg(unix)]
fn verify_private_folder(path: &Path, owner: u32) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() && metadata.uid() == owner && metadata.mode() & 0o077 == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!("{} is not a private folder owned by this user", path.display())))
    }
}

/// 目前使用者的 uid:在 `directory` 建一個自己的檔案、讀它的擁有者再刪掉。
#[cfg(unix)]
fn current_user_id(directory: &Path) -> io::Result<u32> {
    use std::os::unix::fs::MetadataExt;
    let probe = directory.join(format!(".purrdr-owner-probe-{}", random_base36(12)?));
    let file = create_private_file(&probe)?;
    let uid = file.metadata().map(|metadata| metadata.uid());
    drop(file);
    let _ = std::fs::remove_file(&probe);
    uid
}

fn create_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}
