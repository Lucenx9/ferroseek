// SPDX-License-Identifier: GPL-3.0-or-later
//! Local executable commands with explicit registration and bounded output.

mod edit;
mod run;
mod workflow;

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

use crate::matcher;

pub use edit::{change, change_workflow, format_arguments, parse_arguments};
pub use run::{Completion, Job, Report};
pub use workflow::{Plan, Workflow};

const MAX_CONFIG_BYTES: u64 = 256 * 1024;
const MAX_COMMANDS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub parameters: Vec<String>,
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    commands: Vec<Arc<Script>>,
    workflows: Vec<Arc<Workflow>>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    #[serde(default)]
    command: Vec<Script>,
    #[serde(default)]
    workflow: Vec<Workflow>,
}

impl Catalog {
    pub fn load(settings: &Path) -> Self {
        match read_source(&manifest_path(settings)).and_then(|source| parse_manifest(source.as_deref().unwrap_or(""))) {
            Ok(manifest) => Self::from_manifest(manifest),
            Err(error) => Self { error: Some(format!("Check launcher-scripts.toml. {error}")), ..Self::default() },
        }
    }

    fn from_manifest(manifest: Manifest) -> Self {
        Self {
            commands: manifest.command.into_iter().map(Arc::new).collect(),
            workflows: manifest.workflow.into_iter().map(Arc::new).collect(),
            error: None,
        }
    }

    pub fn matching(&self, query: &str, all: bool) -> Vec<(&Arc<Script>, u32)> {
        matching(&self.commands, query, all, |script| (&script.title, &script.description))
    }

    pub fn matching_workflows(&self, query: &str, all: bool) -> Vec<(&Arc<Workflow>, u32)> {
        matching(&self.workflows, query, all, |workflow| (&workflow.title, &workflow.description))
    }

    pub fn plan(&self, workflow: &Workflow) -> Result<Plan, String> {
        Plan::workflow(workflow, &self.commands)
    }

    pub fn commands(&self) -> &[Arc<Script>] {
        &self.commands
    }
}

fn matching<'a, T>(
    entries: &'a [Arc<T>],
    query: &str,
    all: bool,
    label: impl Fn(&T) -> (&str, &str),
) -> Vec<(&'a Arc<T>, u32)> {
    let query = query.trim();
    if query.is_empty() {
        return if all { Vec::new() } else { entries.iter().map(|entry| (entry, 0)).collect() };
    }
    if all && query.chars().take(2).count() < 2 {
        return Vec::new();
    }
    let tokens = matcher::tokens(query);
    entries
        .iter()
        .filter_map(|entry| {
            let (title, description) = label(entry);
            let title = matcher::fold(title);
            let score = if tokens.iter().map(String::as_str).eq(title.split_whitespace()) {
                matcher::EXACT
            } else {
                matcher::score(&tokens, &[(title.as_str(), 100), (&matcher::fold(description), 70)])?
            };
            (!all || score >= 600).then_some((entry, score))
        })
        .collect()
}

fn manifest_path(settings: &Path) -> PathBuf {
    let path = settings.with_file_name("launcher-scripts.toml");
    if path.is_absolute() { path } else { Path::new(".").join(path) }
}

fn read_source(path: &Path) -> io::Result<Option<String>> {
    // A replaced manifest must not block launcher startup if it is a FIFO.
    let file = match OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("Use a regular file."));
    }
    let mut source = String::new();
    file.take(MAX_CONFIG_BYTES + 1).read_to_string(&mut source)?;
    ensure_size(&source)?;
    Ok(Some(source))
}

fn ensure_size(source: &str) -> io::Result<()> {
    if source.len() as u64 > MAX_CONFIG_BYTES {
        return Err(io::Error::other("The file is larger than 256 KiB."));
    }
    Ok(())
}

fn parse_manifest(source: &str) -> io::Result<Manifest> {
    let mut manifest: Manifest =
        toml::from_str(source).map_err(|error| io::Error::other(error.message().to_owned()))?;
    if manifest.command.len() + manifest.workflow.len() > MAX_COMMANDS {
        return Err(io::Error::other("Keep at most 128 commands and workflows in total."));
    }
    let mut titles = HashSet::new();
    for (index, command) in manifest.command.iter_mut().enumerate() {
        command.title = command.title.trim().to_owned();
        validate(command, &mut titles)
            .map_err(|reason| io::Error::other(format!("Command {}. {reason}", index + 1)))?;
    }
    for (index, workflow) in manifest.workflow.iter_mut().enumerate() {
        workflow.title = workflow.title.trim().to_owned();
        workflow
            .validate(&mut titles)
            .map_err(|reason| io::Error::other(format!("Workflow {}. {reason}", index + 1)))?;
    }
    Ok(manifest)
}

fn validate(command: &Script, titles: &mut HashSet<String>) -> Result<(), &'static str> {
    command.validate()?;
    if !titles.insert(matcher::fold(&command.title)) {
        return Err("Use a different title for each command.");
    }
    Ok(())
}

impl Script {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.title.is_empty() || self.title.chars().any(char::is_control) {
            return Err("Enter a single-line title.");
        }
        if !self.executable.is_absolute() || self.executable.file_name().is_none() {
            return Err("Use an absolute path to the executable.");
        }
        validate_parameters(&self.parameters)
    }
}

fn validate_parameters(labels: &[String]) -> Result<(), &'static str> {
    if labels.len() > 3 {
        return Err("Use at most three arguments to fill in at run time.");
    }
    let mut unique = HashSet::new();
    for label in labels {
        let label = label.trim();
        if label.is_empty() || label.chars().any(char::is_control) {
            return Err("Use a single-line name for each argument.");
        }
        if !unique.insert(matcher::fold(label)) {
            return Err("Use a different name for each argument.");
        }
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
            (
                "[[command]]\ntitle = \"Release\"\nexecutable = \"/file\"\nparameters = [\"Project\", \" project \"]",
                "different name",
            ),
        ] {
            std::fs::write(&path, contents).unwrap();
            let catalog = Catalog::load(&settings);
            assert!(catalog.error.as_deref().unwrap().contains(reason));
            assert!(catalog.matching("", false).is_empty());
        }
    }
}
