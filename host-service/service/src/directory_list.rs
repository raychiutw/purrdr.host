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

/// 列出 `path` 的子資料夾。`state_directory` 是主機服務資料夾(一律不列、不進入)。
pub fn list(path: &str, state_directory: &Path) -> Result<Listing, ListError> {
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
    let entries = std::fs::read_dir(&resolved).map_err(|_| ListError::Unreadable)?;

    let mut directories: Vec<String> = entries
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
    let truncated = directories.len() > MAXIMUM_ENTRIES;
    directories.truncate(MAXIMUM_ENTRIES);
    Ok(Listing { directories, truncated })
}
