use std::fs;
use std::path::{Path, PathBuf};

use daw_core::ports::Storage;

/// The shell's filesystem adapter for project documents. Project names are
/// deliberately kept as names, never paths: the project picker belongs in the
/// app and cannot escape its app-data directory.
///
/// The crash-recovery snapshot (issue #15) lives at `snapshot_path`, a
/// sibling of `directory` rather than a file inside it: `list`/`load` only
/// ever look inside `directory`, so the snapshot is structurally incapable
/// of being listed, opened or overwritten as though it were a saved project,
/// whatever a user might name one.
pub struct FileStorage {
    directory: PathBuf,
    snapshot_path: PathBuf,
}

impl FileStorage {
    pub fn new(app_data_dir: PathBuf) -> Self {
        Self {
            directory: app_data_dir.join("projects"),
            snapshot_path: app_data_dir.join("recovery-snapshot.json"),
        }
    }

    fn path_for(&self, name: &str) -> Result<PathBuf, String> {
        let name = name.trim();
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains(['/', '\\'])
            || name.contains('\0')
        {
            return Err("project names must be non-empty names, not paths".into());
        }
        Ok(self.directory.join(format!("{name}.json")))
    }

    fn project_name(path: &Path) -> Option<String> {
        path.extension()
            .is_some_and(|extension| extension == "json")
            .then(|| path.file_stem()?.to_str().map(str::to_owned))?
    }
}

impl Storage for FileStorage {
    fn save(&mut self, name: &str, document: String) -> Result<(), String> {
        let path = self.path_for(name)?;
        fs::create_dir_all(&self.directory)
            .map_err(|error| format!("could not create project directory: {error}"))?;
        fs::write(path, document).map_err(|error| format!("could not save project: {error}"))
    }

    fn load(&self, name: &str) -> Result<Option<String>, String> {
        let path = self.path_for(name)?;
        match fs::read_to_string(path) {
            Ok(document) => Ok(Some(document)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("could not open project: {error}")),
        }
    }

    fn list(&self) -> Result<Vec<String>, String> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("could not list projects: {error}")),
        };
        let mut names = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .file_type()
                    .ok()
                    .filter(|kind| kind.is_file())
                    .map(|_| entry)
            })
            .filter_map(|entry| Self::project_name(&entry.path()))
            .collect::<Vec<_>>();
        names.sort();
        Ok(names)
    }

    fn save_snapshot(&mut self, document: String) -> Result<(), String> {
        if let Some(parent) = self.snapshot_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("could not create app data directory: {error}"))?;
        }
        fs::write(&self.snapshot_path, document)
            .map_err(|error| format!("could not write recovery snapshot: {error}"))
    }

    fn load_snapshot(&self) -> Result<Option<String>, String> {
        match fs::read_to_string(&self.snapshot_path) {
            Ok(document) => Ok(Some(document)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("could not read recovery snapshot: {error}")),
        }
    }

    fn delete_snapshot(&mut self) -> Result<(), String> {
        match fs::remove_file(&self.snapshot_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("could not delete recovery snapshot: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snapshot_file_never_appears_among_saved_project_files() {
        let dir = std::env::temp_dir().join(format!(
            "daw-file-storage-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut storage = FileStorage::new(dir.clone());

        storage
            .save("verse-ideas", "a project document".into())
            .unwrap();
        storage.save_snapshot("a recovery snapshot".into()).unwrap();

        assert_eq!(storage.list().unwrap(), vec!["verse-ideas"]);
        assert_eq!(
            storage.load("verse-ideas").unwrap().unwrap(),
            "a project document"
        );
        assert_eq!(
            storage.load_snapshot().unwrap().unwrap(),
            "a recovery snapshot"
        );
        // The snapshot is a sibling of `projects/`, not a file inside it.
        assert!(dir.join("recovery-snapshot.json").exists());
        assert!(!dir.join("projects").join("recovery-snapshot.json").exists());

        storage.delete_snapshot().unwrap();
        assert_eq!(storage.load_snapshot().unwrap(), None);
        // Deleting the snapshot never touches the saved project file.
        assert_eq!(
            storage.load("verse-ideas").unwrap().unwrap(),
            "a project document"
        );

        let _ = fs::remove_dir_all(dir);
    }
}
