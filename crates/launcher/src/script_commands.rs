// SPDX-License-Identifier: GPL-3.0-or-later
//! Local executable commands with explicit registration and bounded output.

mod run;

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

use crate::matcher;

pub use run::{Completion, Job, Report};

const MAX_CONFIG_BYTES: u64 = 256 * 1024;
const MAX_COMMANDS: usize = 128;

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Default)]
pub struct Catalog {
    commands: Vec<Arc<Script>>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    #[serde(default)]
    command: Vec<Script>,
}

impl Catalog {
    pub fn load(settings: &Path) -> Self {
        let path = settings.with_file_name("launcher-scripts.toml");
        match read_manifest(&path) {
            Ok(commands) => Self { commands: commands.into_iter().map(Arc::new).collect(), error: None },
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(error) => Self { commands: Vec::new(), error: Some(format!("Check launcher-scripts.toml. {error}")) },
        }
    }

    pub fn matching(&self, query: &str, all: bool) -> Vec<(&Arc<Script>, u32)> {
        let query = query.trim();
        if query.is_empty() {
            return if all { Vec::new() } else { self.commands.iter().map(|command| (command, 0)).collect() };
        }
        if all && query.chars().take(2).count() < 2 {
            return Vec::new();
        }
        let tokens = matcher::tokens(query);
        self.commands
            .iter()
            .filter_map(|script| {
                let title = matcher::fold(&script.title);
                let score = if tokens.iter().map(String::as_str).eq(title.split_whitespace()) {
                    matcher::EXACT
                } else {
                    matcher::score(&tokens, &[(title.as_str(), 100), (&matcher::fold(&script.description), 70)])?
                };
                (!all || score >= 600).then_some((script, score))
            })
            .collect()
    }
}

fn read_manifest(path: &Path) -> io::Result<Vec<Script>> {
    // A replaced manifest must not block launcher startup if it is a FIFO.
    let file = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("Use a regular file."));
    }
    let mut source = String::new();
    file.take(MAX_CONFIG_BYTES + 1).read_to_string(&mut source)?;
    if source.len() as u64 > MAX_CONFIG_BYTES {
        return Err(io::Error::other("The file is larger than 256 KiB."));
    }
    let mut manifest: Manifest = toml::from_str(&source).map_err(io::Error::other)?;
    if manifest.command.len() > MAX_COMMANDS {
        return Err(io::Error::other("Keep at most 128 commands."));
    }
    let mut titles = HashSet::new();
    for (index, command) in manifest.command.iter_mut().enumerate() {
        command.title = command.title.trim().to_owned();
        validate(command, &mut titles)
            .map_err(|reason| io::Error::other(format!("Command {}. {reason}", index + 1)))?;
    }
    Ok(manifest.command)
}

fn validate(command: &Script, titles: &mut HashSet<String>) -> Result<(), &'static str> {
    if command.title.is_empty() || command.title.chars().any(char::is_control) {
        return Err("Enter a single-line title.");
    }
    if !command.executable.is_absolute() || command.executable.file_name().is_none() {
        return Err("Use an absolute path to the executable.");
    }
    if !titles.insert(matcher::fold(&command.title)) {
        return Err("Use a different title for each command.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_loads_metadata_without_running_programs() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("config.toml");
        assert!(Catalog::load(&settings).error.is_none());
        std::fs::write(
            dir.path().join("launcher-scripts.toml"),
            r#"
[[command]]
title = "  Release notes  "
description = "Print the current version"
executable = "/synthetic/not-installed"
args = ["argument with spaces", "${HOME}"]
confirm = true
"#,
        )
        .unwrap();
        let catalog = Catalog::load(&settings);
        let matches = catalog.matching("release notes", true);
        let (script, score) = matches[0];
        assert_eq!((script.title.as_str(), score, script.confirm), ("Release notes", matcher::EXACT, true));
        assert_eq!(script.args, ["argument with spaces", "${HOME}"]);
        assert!(catalog.matching("", true).is_empty());
        assert_eq!(catalog.matching("", false).len(), 1);
    }

    #[test]
    fn missing_and_invalid_manifests_are_isolated_from_other_launcher_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("config.toml");
        let path = dir.path().join("launcher-scripts.toml");
        for (contents, reason) in [
            ("[[command]]\ntitle = \"Release\"\nexecutable = \"relative\"", "absolute path"),
            ("[[command]]\ntitle = \"Release\"\nexecutable = \"/file\"\ntypo = true", "unknown field"),
        ] {
            std::fs::write(&path, contents).unwrap();
            let catalog = Catalog::load(&settings);
            assert!(catalog.error.as_deref().unwrap().contains(reason));
            assert!(catalog.matching("", false).is_empty());
        }
    }
}
