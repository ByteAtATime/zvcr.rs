use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub(super) fn discover(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension() == Some(OsStr::new("zvcr3d")) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}
