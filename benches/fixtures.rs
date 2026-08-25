use std::fs;
use std::path::{Path, PathBuf};
use zvcr::Reader;
use zvcr::raw::RegionData;

pub fn collect_region_files(dir: &Path, out: &mut Vec<(u64, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_region_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "zvcr3d") {
            let len = entry.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
            out.push((len, path));
        }
    }
}

pub fn discover_region_file(env_var: &str) -> PathBuf {
    if let Ok(path) = std::env::var(env_var) {
        return PathBuf::from(path);
    }
    let mut candidates: Vec<(u64, PathBuf)> = Vec::new();
    collect_region_files(Path::new("test_files"), &mut candidates);
    assert!(!candidates.is_empty(), "no .zvcr3d files under test_files");
    candidates.sort_unstable();
    candidates
        .into_iter()
        .find_map(|(_, path)| decode_region(&path).is_ok().then_some(path))
        .unwrap_or_else(|| panic!("no decodable .zvcr3d files under test_files"))
}

pub fn decode_region(path: &Path) -> Result<RegionData, String> {
    let bytes = fs::read(path).map_err(|e| format!("failed to read {path:?}: {e}"))?;
    zvcr::ReferenceReader::new(0)
        .from_bytes(&bytes)
        .map_err(|e| format!("failed to decode {path:?}: {e}"))
}
