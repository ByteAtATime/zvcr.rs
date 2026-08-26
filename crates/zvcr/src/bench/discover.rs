use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub fn discover(root: &Path, sample: Option<usize>) -> Vec<PathBuf> {
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
    match sample {
        None => files,
        Some(n) => sample_evenly(files, n),
    }
}

fn sample_evenly(files: Vec<PathBuf>, n: usize) -> Vec<PathBuf> {
    let total = files.len();
    if n >= total {
        return files;
    }
    (0..n)
        .map(|i| i * total / n)
        .map(|idx| files[idx].clone())
        .collect()
}
