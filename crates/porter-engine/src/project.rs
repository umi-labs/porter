use crate::config::ProjectConfig;
use crate::store::Store;
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

const CONFIG_FILE: &str = "porter.toml";
const DB_FILE: &str = "project.db";
/// Everything `clean` may delete. Nothing else in the folder is touched.
const OWNED_DIRS: [&str; 3] = ["raw", "media", "reports"];
const OWNED_FILES: [&str; 3] = [DB_FILE, "project.db-wal", "project.db-shm"];

/// A migration project folder: `porter.toml`, `project.db`, `raw/`, `media/`, `reports/`.
pub struct Project {
    dir: PathBuf,
    config: ProjectConfig,
    store: Store,
}

impl Project {
    /// Writes a minimal commented `porter.toml` and an empty store.
    /// Errors if `porter.toml` already exists.
    pub fn init(dir: &Path, name: &str, adapter: &str) -> Result<Project> {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(CONFIG_FILE);
        let text = template(name, adapter);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("{} already exists or cannot be created", path.display()))?;
        std::io::Write::write_all(&mut file, text.as_bytes())?;
        Project::open(dir)
    }

    pub fn open(dir: &Path) -> Result<Project> {
        let path = dir.join(CONFIG_FILE);
        if !path.exists() {
            bail!("no {CONFIG_FILE} in {}", dir.display());
        }
        let config = ProjectConfig::load(&path)?;
        let store = Store::open(&dir.join(DB_FILE), dir.join("media"))?;
        Ok(Project {
            dir: dir.to_path_buf(),
            config,
            store,
        })
    }

    /// Deletes `raw/`, `media/`, `reports/` and `project.db` only.
    /// The store is closed; reopen the project to use it again.
    pub fn clean(&self) -> Result<()> {
        self.store.close()?;
        for d in OWNED_DIRS {
            let p = self.dir.join(d);
            if p.exists() {
                fs::remove_dir_all(&p).with_context(|| format!("removing {}", p.display()))?;
            }
        }
        for f in OWNED_FILES {
            let p = self.dir.join(f);
            if p.exists() {
                fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
            }
        }
        Ok(())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn config(&self) -> &ProjectConfig {
        &self.config
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn raw_dir(&self, connector: &str) -> PathBuf {
        self.dir.join("raw").join(connector)
    }

    pub fn media_dir(&self) -> PathBuf {
        self.dir.join("media")
    }

    pub fn reports_dir(&self) -> PathBuf {
        self.dir.join("reports")
    }
}

fn template(name: &str, adapter: &str) -> String {
    // Serialise the user-supplied values so quotes and backslashes are escaped.
    let quote = |s: &str| toml::Value::String(s.to_string()).to_string();
    let name = quote(name);
    let adapter = quote(adapter);
    format!(
        r#"# Porter project. Credentials never go here: name the env var that holds them.

[project]
name = {name}

[source]
adapter = {adapter}

# One [[connectors]] block per way of reading the source, e.g.:
# [[connectors]]
# kind = "crawl"

[destination]
# adapter = ""
# url = "https://cms.example.com"
# api_key_env = "MY_API_KEY_ENV_VAR"
# auth_collection = "users"

# [mapping]

# [[redirects]]
# from = "/old-path"
# to = "/new-path"
# code = 301

# [verify]
# site_url = "https://www.example.com"
"#
    )
}
