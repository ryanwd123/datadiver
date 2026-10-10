use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_DEPTH: usize = 8;
const MAX_FILES: usize = 20_000;

pub fn scan_sources(folders: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for folder in folders {
        if files.len() >= MAX_FILES {
            break;
        }
        let root = folder.canonicalize().unwrap_or_else(|_| folder.clone());
        scan_directory(&root, 0, &mut seen, &mut files);
    }
    files.sort_by(|left, right| {
        left.to_string_lossy()
            .to_lowercase()
            .cmp(&right.to_string_lossy().to_lowercase())
    });
    files
}

fn scan_directory(
    directory: &Path,
    depth: usize,
    seen: &mut HashSet<String>,
    files: &mut Vec<PathBuf>,
) {
    if depth > MAX_DEPTH || files.len() >= MAX_FILES {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if files.len() >= MAX_FILES {
            break;
        }
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            scan_directory(&path, depth + 1, seen, files);
        } else if metadata.is_file() && supported_source(&path) {
            let canonical = path.canonicalize().unwrap_or(path);
            let identity = normalized_identity(&canonical);
            if seen.insert(identity) {
                files.push(canonical);
            }
        }
    }
}

fn supported_source(path: &Path) -> bool {
    super::source_database::source_reader(path).is_ok()
}

fn normalized_identity(path: &Path) -> String {
    let identity = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        identity.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        identity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_deduplicates_overlapping_folders_and_skips_hidden_entries() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("nested")).unwrap();
        fs::create_dir(temp.path().join(".hidden")).unwrap();
        for name in [
            "a.CSV",
            "nested/b.pq",
            "nested/c.txt",
            "nested/report.CSV.GZ",
            "nested/other.txt.gz",
            ".hidden/d.csv",
            ".secret.csv",
        ] {
            let path = temp.path().join(name);
            fs::write(path, b"").unwrap();
        }
        let files = scan_sources(&[temp.path().to_path_buf(), temp.path().join("nested")]);
        assert_eq!(files.len(), 3);
        assert!(files[0].ends_with("a.CSV"));
        assert!(files[1].ends_with("b.pq"));
        assert!(files[2].ends_with("report.CSV.GZ"));
    }
}
