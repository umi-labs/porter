//! Recently opened projects, newest first, kept in `<config dir>/porter/recent.json`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const MAX: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentProject {
    pub path: PathBuf,
}

fn file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("porter").join("recent.json"))
}

pub fn load() -> Vec<RecentProject> {
    file().map(|f| load_from(&f)).unwrap_or_default()
}

/// Moves `path` to the top of the list. A failure to save only costs the list, so it is logged.
pub fn push(path: &Path) {
    let Some(f) = file() else {
        eprintln!("porter-desktop: no config directory; recent projects not saved");
        return;
    };
    if let Err(e) = push_to(&f, path) {
        eprintln!("porter-desktop: could not save {}: {e:#}", f.display());
    }
}

fn load_from(file: &Path) -> Vec<RecentProject> {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            eprintln!("porter-desktop: could not read {}: {e}", file.display());
            return Vec::new();
        }
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        eprintln!("porter-desktop: ignoring corrupt {}: {e}", file.display());
        Vec::new()
    })
}

fn push_to(file: &Path, path: &Path) -> anyhow::Result<()> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut list = load_from(file);
    list.retain(|r| r.path != path);
    list.insert(0, RecentProject { path });
    list.truncate(MAX);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&list)?)?;
    std::fs::rename(&tmp, file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_dedupes_and_caps() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("porter/recent.json");
        for i in 0..25 {
            push_to(&file, Path::new(&format!("/p/{i}"))).unwrap();
        }
        push_to(&file, Path::new("/p/10")).unwrap();
        let list = load_from(&file);
        assert_eq!(list.len(), MAX);
        assert_eq!(list[0].path, Path::new("/p/10"));
        assert_eq!(list[1].path, Path::new("/p/24"));
        assert_eq!(
            list.iter().filter(|r| r.path == Path::new("/p/10")).count(),
            1
        );
        assert!(!list.iter().any(|r| r.path == Path::new("/p/0")));
    }

    #[test]
    fn recent_survives_corrupt_file() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("recent.json");
        std::fs::write(&file, "{not json").unwrap();
        assert!(load_from(&file).is_empty());
        push_to(&file, Path::new("/p/a")).unwrap();
        assert_eq!(
            load_from(&file),
            vec![RecentProject {
                path: "/p/a".into()
            }]
        );
    }
}
