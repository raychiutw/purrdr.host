//! 資料夾瀏覽(raychiutw/purrdr.host#26):`directory.list` 只回一個資料夾裡的子資料夾名稱,不回檔案與內容。
//! 排除清單與主機取檔共用(`host_files::is_excluded`);symlink 先解析再比對，排除的子項目直接不列。

use std::path::{Path, PathBuf};

use crate::host_files;

/// 一次最多回傳的子資料夾數;多的以 `truncated` 告知。
pub const MAXIMUM_ENTRIES: usize = 1000;

#[derive(Debug, PartialEq, Eq)]
pub struct Listing {
    pub directories: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ListError {
    /// 不是乾淨的絕對路徑(相對路徑、`.`／`..` 段落、控制字元)。
    InvalidPath,
    NotFound,
    /// 資料夾本身在排除清單或是主機服務資料夾。
    NotAllowed,
    Unreadable,
}

/// 最多讀幾個目錄項目就停(資料夾可能有幾十萬個項目):限制的是工作量，不只是回覆大小。
const MAXIMUM_SCANNED_ENTRIES: usize = 10_000;

/// 列出 `path` 的子資料夾。`state_directory` 是主機服務資料夾(一律不列、不進入)。
pub fn list(path: &str, state_directory: &Path) -> Result<Listing, ListError> {
    list_with_scan_limit(path, state_directory, MAXIMUM_SCANNED_ENTRIES)
}

fn list_with_scan_limit(path: &str, state_directory: &Path, scan_limit: usize) -> Result<Listing, ListError> {
    if !host_files::is_plain_absolute(path) {
        return Err(ListError::InvalidPath);
    }
    let requested = Path::new(path);
    let state = host_files::resolve(state_directory).unwrap_or_else(|| state_directory.to_owned());
    let forbidden = |candidate: &Path| host_files::is_excluded(candidate) || host_files::is_under_or_same(candidate, &state);

    if forbidden(requested) {
        return Err(ListError::NotAllowed);
    }
    let resolved = host_files::resolve(requested).filter(|resolved| resolved.is_dir()).ok_or(ListError::NotFound)?;
    if forbidden(&resolved) {
        return Err(ListError::NotAllowed);
    }
    let mut entries = std::fs::read_dir(&resolved).map_err(|_| ListError::Unreadable)?;

    let scanned: Vec<_> = entries.by_ref().take(scan_limit).collect();
    // 還有沒讀的項目:結果不完整。
    let scan_truncated = entries.next().is_some();
    let mut directories: Vec<String> = scanned
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let child: PathBuf = resolved.join(&name);
            let target = if entry.file_type().ok()?.is_symlink() {
                host_files::resolve(&child)?
            } else {
                child.clone()
            };
            (target.is_dir() && !forbidden(&child) && !forbidden(&target)).then_some(name)
        })
        .collect();
    directories.sort_by(|left, right| left.to_lowercase().cmp(&right.to_lowercase()).then_with(|| left.cmp(right)));
    let truncated = scan_truncated || directories.len() > MAXIMUM_ENTRIES;
    directories.truncate(MAXIMUM_ENTRIES);
    Ok(Listing { directories, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanning_stops_at_the_scan_limit_and_reports_truncated() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..20 {
            std::fs::create_dir(root.path().join(format!("d{index:02}"))).unwrap();
        }
        let state = tempfile::tempdir().unwrap();

        let listing = list_with_scan_limit(root.path().to_str().unwrap(), state.path(), 5).unwrap();

        assert!(listing.truncated);
        assert_eq!(listing.directories.len(), 5, "只掃前 5 個項目");
        let all = list_with_scan_limit(root.path().to_str().unwrap(), state.path(), 20).unwrap();
        assert!(!all.truncated);
        assert_eq!(all.directories.len(), 20);
    }
}
