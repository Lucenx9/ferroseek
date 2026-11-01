// SPDX-License-Identifier: GPL-3.0-or-later
//! Folder-only edits preserve the rest of the user's configuration.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::{Config, expand, normalize};

#[derive(Debug, Clone)]
pub struct Document {
    pub path: PathBuf,
    pub config: Config,
    source: Option<String>,
}

impl Document {
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let path =
            if path.is_absolute() { path } else { std::env::current_dir().map_err(|e| e.to_string())?.join(path) };
        let source = read(&path)?;
        let config = match &source {
            Some(text) => Config::parse(text).map_err(|e| e.to_string())?,
            None => Config::default(),
        };
        Ok(Self { path, config, source })
    }

    pub fn is_new(&self) -> bool {
        self.source.is_none()
    }

    /// Save only roots, refusing a stale snapshot or a symbolic-link file.
    /// Existing unavailable roots can be retained, e.g. an unplugged disk.
    pub fn save(&self, roots: Vec<String>) -> Result<Self, String> {
        let previous = if self.is_new() { Vec::new() } else { self.config.root_paths() };
        for root in &roots {
            let path = validate_path(root)?;
            if !previous.contains(&path) {
                validate_directory(&path)?;
            }
        }
        let dir = self.path.parent().ok_or("Configuration has no parent folder")?;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| e.to_string())?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.path.with_extension("toml.lock"))
            .map_err(|e| e.to_string())?;
        lock.try_lock().map_err(|_| "Settings are busy. Try again.".to_string())?;
        if std::fs::symlink_metadata(&self.path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("Configuration is a symbolic link. Its target must be edited separately.".into());
        }
        if read(&self.path)? != self.source {
            return Err("Settings changed on disk. Reload before saving; your draft has been kept.".into());
        }
        let mut document =
            self.source.as_deref().unwrap_or("").parse::<toml_edit::DocumentMut>().map_err(|e| e.to_string())?;
        let mut array = toml_edit::Array::new();
        for root in &roots {
            array.push(root.as_str());
        }
        let decor = document.get("roots").and_then(toml_edit::Item::as_value).map(|value| value.decor().clone());
        document["roots"] = toml_edit::value(array);
        if let Some(decor) = decor {
            if let Some(value) = document["roots"].as_value_mut() {
                *value.decor_mut() = decor;
            }
        }
        let text = document.to_string();
        let config = Config::parse(&text).map_err(|e| e.to_string())?;
        let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
        temp.write_all(text.as_bytes()).and_then(|_| temp.as_file().sync_all()).map_err(|e| e.to_string())?;
        // Recheck after preparing the new file as well as under the app lock.
        if read(&self.path)? != self.source {
            return Err("Settings changed on disk. Reload before saving.".into());
        }
        temp.persist(&self.path).map_err(|e| e.to_string())?;
        std::fs::File::open(dir).and_then(|f| f.sync_all()).map_err(|e| {
            format!("Settings were written but durability could not be confirmed. Reload before retrying: {e}")
        })?;
        Ok(Self { path: self.path.clone(), config, source: Some(text) })
    }
}

fn read(path: &Path) -> Result<Option<String>, String> {
    if std::fs::metadata(path).is_ok_and(|m| !m.is_file()) {
        return Err("Configuration must be a regular file.".into());
    }
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Cannot read settings: {e}")),
    }
}

fn validate_path(root: &str) -> Result<PathBuf, String> {
    if root.trim().is_empty() || root.contains(['\0', '\n', '\r']) {
        return Err("Enter a folder path.".into());
    }
    let path = expand(root);
    if !path.is_absolute() {
        return Err("Use an absolute folder path or ~/ followed by a folder name.".into());
    }
    Ok(normalize(&path))
}

fn validate_directory(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| format!("Folder is not available: {e}"))?;
    if metadata.file_type().is_symlink() {
        return Err("This is a symbolic link. Choose the destination folder instead.".into());
    }
    if !metadata.is_dir() {
        return Err("This path is not a folder.".into());
    }
    std::fs::read_dir(path).map_err(|e| format!("Folder cannot be read: {e}"))?;
    Ok(())
}

pub fn validate_folder(root: &str) -> Result<(), String> {
    validate_directory(&validate_path(root)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn folder_edits_preserve_settings_comments_and_conflicts() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let root = temp.path().join("Projects:2026 é");
        std::fs::create_dir(&root).unwrap();
        let original = "# My settings\nroots = [] # folders\nmax_cpu_percent = 17\nindex_hidden = true\nexclude_names = ['private*']\n";
        std::fs::write(&path, original).unwrap();
        let doc = Document::load(path.clone()).unwrap();
        let saved = doc.save(vec![root.to_str().unwrap().into()]).unwrap();
        assert_eq!(saved.config.max_cpu_percent, 17);
        assert!(saved.config.index_hidden);
        assert_eq!(saved.config.exclude_names, ["private*"]);
        assert!(std::fs::read_to_string(&path).unwrap().contains("# My settings"));
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(doc.save(vec![]).unwrap_err().contains("changed on disk"));
        assert_eq!(Config::load(&path).unwrap(), saved.config);
        let cleared = saved.save(vec![]).unwrap();
        assert!(cleared.config.roots.is_empty());
    }

    #[test]
    fn first_run_validation_and_unavailable_existing_roots() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings/config.toml");
        let folder = temp.path().join("work");
        std::fs::create_dir(&folder).unwrap();
        let doc = Document::load(path.clone()).unwrap();
        assert!(doc.is_new());
        assert!(doc.save(vec![temp.path().join("missing").to_str().unwrap().into()]).is_err());
        assert!(!path.exists());
        let saved = doc.save(vec![folder.to_str().unwrap().into()]).unwrap();
        assert!(!saved.is_new());
        std::fs::remove_dir(&folder).unwrap();
        assert!(saved.save(saved.config.roots.clone()).is_ok(), "offline roots can be retained");
        assert!(validate_folder("relative").is_err());
        let file = temp.path().join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(validate_folder(file.to_str().unwrap()).unwrap_err().contains("not a folder"));
        let link = temp.path().join("link");
        symlink(temp.path(), &link).unwrap();
        assert!(validate_folder(link.to_str().unwrap()).unwrap_err().contains("symbolic link"));
    }

    #[test]
    fn invalid_and_linked_configurations_are_not_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "unknown = true").unwrap();
        assert!(Document::load(path.clone()).is_err());
        std::fs::write(&path, "roots = []").unwrap();
        let link = temp.path().join("link.toml");
        symlink(&path, &link).unwrap();
        assert!(Document::load(link).unwrap().save(vec![]).unwrap_err().contains("symbolic link"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "roots = []");
    }
}
