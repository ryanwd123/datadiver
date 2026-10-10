use std::{
    fs,
    path::{Path, PathBuf},
};

const MAX_ENTRIES: usize = 500;

/// Where DuckDB looks for the files a query names, read from the SQL connection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PathRoots {
    pub home: Option<PathBuf>,
    /// `file_search_path` entries, tried in order before the working directory.
    pub search: Vec<PathBuf>,
    pub working: Option<PathBuf>,
}

impl PathRoots {
    /// `home_directory` falls back to the user profile when unset, as DuckDB does.
    pub fn new(file_search_path: &str, home_directory: &str) -> Self {
        let home = Some(home_directory)
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("USERPROFILE")
                    .or_else(|| std::env::var_os("HOME"))
                    .map(PathBuf::from)
            });
        let search = file_search_path
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| expand_home(entry, home.as_deref()).unwrap_or_else(|| entry.into()))
            .collect();
        Self {
            home,
            search,
            working: std::env::current_dir().ok(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathCandidate {
    pub name: String,
    pub is_dir: bool,
    pub detail: String,
}

fn expand_home(path: &str, home: Option<&Path>) -> Option<PathBuf> {
    let rest = path.strip_prefix('~')?;
    if !(rest.is_empty() || rest.starts_with(['/', '\\'])) {
        return None;
    }
    Some(home?.join(rest.trim_start_matches(['/', '\\'])))
}

/// Folders and files whose name starts with the last segment of `typed`.
///
/// A relative path is looked up in every search root and the working
/// directory; the root is shown as detail so same-named folders are told apart.
pub fn list(typed: &str, roots: &PathRoots) -> Vec<PathCandidate> {
    let split = typed.rfind(['/', '\\']).map_or(0, |at| at + 1);
    let (directory, prefix) = typed.split_at(split);
    let prefix = prefix.to_lowercase();
    let path = Path::new(directory);
    let bases: Vec<(PathBuf, Option<&Path>)> =
        if let Some(home) = expand_home(directory, roots.home.as_deref()) {
            vec![(home, None)]
        } else if path.has_root() || path.is_absolute() {
            vec![(path.to_path_buf(), None)]
        } else {
            roots
                .search
                .iter()
                .chain(&roots.working)
                .map(|root| (root.join(directory), Some(root.as_path())))
                .collect()
        };
    let several_roots = bases.len() > 1;
    let mut candidates: Vec<PathCandidate> = Vec::new();
    for (base, root) in bases {
        let Ok(entries) = fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if (name.starts_with('.') && !prefix.starts_with('.'))
                || !name.to_lowercase().starts_with(&prefix)
                || candidates.iter().any(|c| c.name == name)
            {
                continue;
            }
            // Follows links, so a linked folder lists as a folder.
            let metadata = fs::metadata(entry.path()).ok();
            let is_dir = metadata.as_ref().is_some_and(|m| m.is_dir());
            let mut detail = Vec::new();
            if !is_dir && let Some(metadata) = &metadata {
                detail.push(file_size(metadata.len()));
            }
            if several_roots && let Some(root) = root {
                detail.push(root.display().to_string());
            }
            candidates.push(PathCandidate {
                name,
                is_dir,
                detail: detail.join(" · "),
            });
            if candidates.len() >= MAX_ENTRIES {
                break;
            }
        }
    }
    candidates.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    candidates
}

fn file_size(bytes: u64) -> String {
    let mut size = bytes as f64;
    for unit in ["B", "KB", "MB", "GB"] {
        if size < 1024. || unit == "GB" {
            return if unit == "B" {
                format!("{bytes} B")
            } else {
                format!("{size:.1} {unit}")
            };
        }
        size /= 1024.;
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(candidates: &[PathCandidate]) -> Vec<&str> {
        candidates.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn lists_folders_first_filters_by_prefix_and_hides_dot_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("Sales")).unwrap();
        fs::write(dir.path().join("sales.csv"), "a\n1\n").unwrap();
        fs::write(dir.path().join("apples.parquet"), "").unwrap();
        fs::write(dir.path().join(".hidden"), "").unwrap();
        let roots = PathRoots::default();
        let typed = format!("{}/", dir.path().display());
        let all = list(&typed, &roots);
        assert_eq!(names(&all), ["Sales", "apples.parquet", "sales.csv"]);
        assert!(all[0].is_dir);
        assert_eq!(all[2].detail, "4 B");
        assert_eq!(
            names(&list(&format!("{typed}SA"), &roots)),
            ["Sales", "sales.csv"]
        );
        assert_eq!(names(&list(&format!("{typed}."), &roots)), [".hidden"]);
    }

    #[test]
    fn relative_paths_merge_search_roots_and_home_expands() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        fs::create_dir_all(first.path().join("data")).unwrap();
        fs::create_dir_all(second.path().join("data")).unwrap();
        fs::write(first.path().join("data/a.csv"), "").unwrap();
        fs::write(second.path().join("data/a.csv"), "").unwrap();
        fs::write(second.path().join("data/b.csv"), "").unwrap();
        let search_path = format!("{}, {}", first.path().display(), second.path().display());
        let roots = PathRoots {
            working: None,
            ..PathRoots::new(&search_path, &second.path().display().to_string())
        };
        let found = list("data/", &roots);
        assert_eq!(names(&found), ["a.csv", "b.csv"]);
        assert!(
            found[0]
                .detail
                .ends_with(&first.path().display().to_string())
        );
        assert_eq!(names(&list("~/data/b", &roots)), ["b.csv"]);
        assert_eq!(names(&list("./data/b", &roots)), ["b.csv"]);
    }
}
