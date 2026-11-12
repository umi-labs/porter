use anyhow::Result;
use porter_engine::project::Project;
use std::path::Path;

pub mod clean;
pub mod export;
pub mod init;
pub mod issues;
pub mod report;
pub mod stage;
pub mod status;

pub fn open(dir: &Path) -> Result<Project> {
    tracing::debug!(dir = %dir.display(), "opening project");
    Project::open(dir)
}
