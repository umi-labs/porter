use std::fs::{File, create_dir_all};
use std::io::{Read, Write};
use std::path::Path;
use anyhow::{Result, anyhow};

/// Ensures a directory exists, creating it if necessary
pub fn ensure_dir(dir: &str) -> Result<()> {
    let path = Path::new(dir);
    if !path.exists() {
        create_dir_all(path)?;
    }
    Ok(())
}

/// Reads a file to a string
pub fn read_file(path: &str) -> Result<String> {
    let mut file = File::open(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok(contents)
}

/// Writes a string to a file
pub fn write_file(path: &str, contents: &str) -> Result<()> {
    let dir = Path::new(path).parent().ok_or_else(|| anyhow!("Invalid path"))?;
    ensure_dir(dir.to_str().ok_or_else(|| anyhow!("Invalid directory path"))?)?;
    let mut file = File::create(path)?;
    file.write_all(contents.as_bytes())?;
    Ok(())
}

/// Checks if a file exists
pub fn file_exists(path: &str) -> bool {
    Path::new(path).exists()
}

/// Deletes a file
pub fn delete_file(path: &str) -> Result<()> {
    std::fs::remove_file(path)?;
    Ok(())
}
