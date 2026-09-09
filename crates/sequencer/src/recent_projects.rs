//! Bounded, persistent history of successfully opened or saved projects.
use std::{io, path::Path, sync::Mutex};
static WRITE_LOCK: Mutex<()> = Mutex::new(());
const LIMIT: usize = 12;

pub fn list() -> io::Result<Vec<String>> {
    read(
        &crate::app_paths::app_paths()
            .user_data_root()
            .join("recent-projects.json"),
    )
}
fn read(path: &Path) -> io::Result<Vec<String>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(vec![]),
        Err(error) => Err(error),
    }
}
pub fn record(name: &str) -> io::Result<()> {
    let _lock = WRITE_LOCK.lock().unwrap();
    let path = crate::app_paths::app_paths()
        .user_data_root()
        .join("recent-projects.json");
    record_at(&path, name)
}
fn record_at(path: &Path, name: &str) -> io::Result<()> {
    let mut names = read(path)?;
    names.retain(|entry| entry != name);
    names.insert(0, name.to_string());
    names.truncate(LIMIT);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec(&names)?)?;
    std::fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recent_projects_are_bounded_deduplicated_and_persistent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent.json");
        for index in 0..20 {
            record_at(&path, &format!("Project {index}")).unwrap();
        }
        record_at(&path, "Project 12").unwrap();
        let entries = read(&path).unwrap();
        assert_eq!(entries.len(), LIMIT);
        assert_eq!(entries[0], "Project 12");
        assert_eq!(
            entries.iter().filter(|name| *name == "Project 12").count(),
            1
        );
    }
}
