mod issues;
mod items;
mod media;
mod runs;

use anyhow::{Context, Result, anyhow};
use rusqlite::Connection;
use serde::{Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use uuid::Uuid;

pub use issues::IssueFilter;
pub use media::MediaHandle;
pub use runs::RunRow;

const SCHEMA: &str = include_str!("schema.sql");
const SCHEMA_V2: &str = include_str!("schema_v2.sql");
const SCHEMA_VERSION: i64 = 2;
const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";

/// Id of one stage run (UUIDv7).
#[derive(Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct RunId(pub Uuid);

/// The project database. One connection behind a mutex, WAL mode.
pub struct Store {
    conn: Mutex<Connection>,
    media_dir: PathBuf,
}

impl Store {
    pub fn open(path: &Path, media_dir: PathBuf) -> Result<Store> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        match version {
            0 => conn.execute_batch(&format!("BEGIN; {SCHEMA} {SCHEMA_V2} COMMIT;"))?,
            1 => conn.execute_batch(&format!("BEGIN; {SCHEMA_V2} COMMIT;"))?,
            SCHEMA_VERSION => {}
            v => {
                return Err(anyhow!(
                    "{} has schema version {v}; this porter supports {SCHEMA_VERSION}",
                    path.display()
                ));
            }
        }
        Ok(Store {
            conn: Mutex::new(conn),
            media_dir,
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock can't leave SQLite inconsistent, so recover.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Drops the file connection so the database files can be deleted.
    pub(crate) fn close(&self) -> Result<()> {
        let mut c = self.conn();
        let old = std::mem::replace(&mut *c, Connection::open_in_memory()?);
        old.close().map_err(|(_, e)| e)?;
        Ok(())
    }
}

/// Wire string of a unit enum (or tag of a data-carrying one), e.g. `Stage::Load` -> `"load"`.
fn wire<T: Serialize>(v: &T) -> Result<String> {
    match serde_json::to_value(v)? {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Object(m) => m
            .keys()
            .next()
            .cloned()
            .ok_or_else(|| anyhow!("empty enum object")),
        other => Err(anyhow!("not an enum: {other}")),
    }
}

fn from_wire<T: DeserializeOwned>(s: String) -> Result<T> {
    Ok(serde_json::from_value(serde_json::Value::String(s))?)
}
