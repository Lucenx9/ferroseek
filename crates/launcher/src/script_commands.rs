// SPDX-License-Identifier: GPL-3.0-or-later
//! Local executable commands with explicit registration and bounded output.

mod edit;
mod run;
mod workflow;

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::matcher;

pub use edit::{change, change_workflow, format_arguments, parse_arguments};
pub use run::{Completion, Job, Report};
pub use workflow::{Plan, Workflow};

const MAX_CONFIG_BYTES: u64 = 256 * 1024;
const MAX_COMMANDS: usize = 128;
const SCRIPT_KEY: &str = "script-v1";
const WORKFLOW_KEY: &str = "workflow-v1";

pub(crate) fn is_definition_key(key: &str) -> bool {
    key.split_once(':').is_some_and(|(kind, digest)| {
        matches!(kind, SCRIPT_KEY | WORKFLOW_KEY)
            && digest.len() == 64
            && digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

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
    history: HashMap<String, HistoryEntry>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum HistoryEntry {
    Script(Arc<Script>),
    Workflow(Arc<Workflow>),
}

impl HistoryEntry {
    fn title(&self) -> &str {
        match self {
            Self::Script(script) => &script.title,
            Self::Workflow(workflow) => &workflow.title,
        }
    }
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
        let commands: Vec<_> = manifest.command.into_iter().map(Arc::new).collect();
        let workflows: Vec<_> = manifest.workflow.into_iter().map(Arc::new).collect();
        let history = history_entries(&commands, &workflows);
        Self { commands, workflows, history, error: None }
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

    pub(crate) fn history_entry(&self, key: &str) -> Option<&HistoryEntry> {
        self.history.get(key)
    }

    pub(crate) fn shortcut_plan(&self, key: &str) -> Result<Plan, &'static str> {
        let mut plan = match self.history_entry(key) {
            Some(HistoryEntry::Script(script)) => Plan::single(Arc::clone(script)),
            Some(HistoryEntry::Workflow(workflow)) => self
                .plan(workflow)
                .map_err(|_| "This workflow is unavailable. Check its steps and update the shortcut.")?,
            None => return Err("Shortcut unavailable. Reassign it."),
        };
        plan.confirm = true;
        Ok(plan)
    }

    pub(crate) fn history_key(&self, title: &str) -> Option<&str> {
        self.history.iter().find(|(_, entry)| entry.title() == title).map(|(key, _)| key.as_str())
    }
}

fn history_entries(commands: &[Arc<Script>], workflows: &[Arc<Workflow>]) -> HashMap<String, HistoryEntry> {
    let scripts: HashMap<_, _> = commands
        .iter()
        .filter_map(|script| Some((matcher::fold(&script.title), (script.history_key()?, script))))
        .collect();
    let workflow_entries = workflows.iter().filter_map(|workflow| {
        let ordered = workflow.resolve_steps(|title| scripts.get(title).map(|(key, _)| key.as_str())).ok()?;
        let key = history_key(WORKFLOW_KEY, (&workflow.title, ordered))?;
        Some((key, HistoryEntry::Workflow(Arc::clone(workflow))))
    });
    scripts
        .values()
        .map(|(key, script)| (key.clone(), HistoryEntry::Script(Arc::clone(script))))
        .chain(workflow_entries)
        .collect()
}

fn history_key(prefix: &str, definition: impl Serialize) -> Option<String> {
    let definition = serde_json::to_vec(&definition).ok()?;
    Some(format!("{prefix}:{:x}", Sha256::digest(definition)))
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
    fn history_key(&self) -> Option<String> {
        history_key(SCRIPT_KEY, (&self.title, &self.executable, &self.args, &self.parameters, self.confirm))
    }

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
    fn changing_a_step_retires_its_history_and_the_dependent_workflow() {
        let source = r#"
[[command]]
title = "Build"
executable = "/synthetic/build"
args = ["--old"]

[[command]]
title = "Check"
executable = "/synthetic/check"

[[workflow]]
title = "Prepare"
steps = [" build ", "CHECK"]

[[workflow]]
title = "Unavailable"
steps = ["Build", "Missing"]
"#;
        let before = Catalog::from_manifest(parse_manifest(source).unwrap());
        let after = Catalog::from_manifest(parse_manifest(&source.replace("--old", "--new")).unwrap());
        for title in ["Build", "Prepare"] {
            assert!(after.history_key(title).is_some(), "{title} still has an executable definition");
            assert!(after.history_entry(before.history_key(title).unwrap()).is_none(), "{title} must start fresh");
        }
        assert_eq!(before.history_key("Check"), after.history_key("Check"));
        assert!(after.history_key("Unavailable").is_none());
    }

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
