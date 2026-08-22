use std::fs;
use std::path::{Path, PathBuf};

use daw_core::ports::Storage;

/// The shell's filesystem adapter for project documents. Project names are
/// deliberately kept as names, never paths: the project picker belongs in the
/// app and cannot escape its app-data directory.
pub struct FileStorage {
    directory: PathBuf,
}

impl FileStorage {
    pub fn new(app_data_dir: PathBuf) -> Self {
        Self {
            directory: app_data_dir.join("projects"),
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
}
