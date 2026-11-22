// SPDX-License-Identifier: GPL-3.0-or-later
//! Comment-preserving writes to the local command manifest.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value};

use super::{Catalog, Manifest, Script, Workflow, ensure_size, manifest_path, parse_manifest, read_source};
use crate::usage;

pub fn format_arguments(args: &[String]) -> String {
    argument_array(args).to_string()
}

pub fn parse_arguments(field: &str) -> Result<Vec<String>, String> {
    if field.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value = field.parse::<Value>().map_err(|_| "Use a quoted list for arguments, such as [\"--summary\"].")?;
    value
        .as_array()
        .ok_or("Use a quoted list for arguments.")?
        .iter()
        .map(|entry| entry.as_str().map(str::to_owned).ok_or_else(|| "Write each argument in quotes.".into()))
        .collect()
}

fn argument_array(args: &[String]) -> Array {
    args.iter().map(|arg| Value::from(arg.as_str())).collect()
}

#[derive(Debug, Clone)]
pub struct Saved {
    pub catalog: Catalog,
    pub warning: Option<String>,
}

pub fn change(settings: &Path, previous: Option<&Script>, next: Option<Script>) -> Result<Saved, String> {
    change_manifest(settings, |manifest, document| {
        let index = existing_index(&manifest.command, previous, "command")?;
        check_references(&manifest.workflow, previous, next.as_ref())?;
        let list = document.as_table_mut().entry("command").or_insert(Item::ArrayOfTables(ArrayOfTables::new()));
        update_list(list, index, previous, next.as_ref())
    })
}

pub fn change_workflow(settings: &Path, previous: Option<&Workflow>, next: Option<Workflow>) -> Result<Saved, String> {
    change_manifest(settings, |manifest, document| {
        let index = existing_index(&manifest.workflow, previous, "workflow")?;
        if let Some(next) = &next {
            Catalog::from_manifest(manifest).plan(next).map_err(io::Error::other)?;
        }
        let list = document.as_table_mut().entry("workflow").or_insert(Item::ArrayOfTables(ArrayOfTables::new()));
        update_list(list, index, previous, next.as_ref())
    })
}

fn existing_index<T: PartialEq>(entries: &[T], previous: Option<&T>, name: &str) -> io::Result<Option<usize>> {
    previous
        .map(|old| {
            entries
                .iter()
                .position(|entry| entry == old)
                .ok_or_else(|| io::Error::other(format!("This {name} changed on disk. Reopen FerroSeek to load it.")))
        })
        .transpose()
}

fn change_manifest(
    settings: &Path,
    update: impl FnOnce(Manifest, &mut DocumentMut) -> io::Result<()>,
) -> Result<Saved, String> {
    let path = manifest_path(settings);
    usage::with_lock(&path, false, || edit(&path, update)).map_err(|error| match error.kind() {
        io::ErrorKind::WouldBlock => "The command file is busy. Try again.".into(),
        _ => error.to_string(),
    })
}

fn edit(path: &Path, update: impl FnOnce(Manifest, &mut DocumentMut) -> io::Result<()>) -> io::Result<Saved> {
    revision(path)?;
    let source = read_source(path)?;
    let original = source.as_deref().unwrap_or("");
    let manifest = parse_manifest(original)?;
    let mut document = original.parse::<DocumentMut>().map_err(|error| io::Error::other(error.message().to_owned()))?;
    update(manifest, &mut document)?;
    let text = document.to_string();
    ensure_size(&text)?;
    let manifest = parse_manifest(&text)?;
    let warning = if text != original { persist(path, source.as_deref(), &text)? } else { None };
    Ok(Saved { catalog: Catalog::from_manifest(manifest), warning })
}

fn check_references(workflows: &[super::Workflow], old: Option<&Script>, next: Option<&Script>) -> io::Result<()> {
    let Some(old) = old else { return Ok(()) };
    let title = crate::matcher::fold(&old.title);
    if next.is_some_and(|next| crate::matcher::fold(&next.title) == title) {
        return Ok(());
    }
    if let Some(workflow) =
        workflows.iter().find(|workflow| workflow.steps.iter().any(|step| crate::matcher::fold(step.trim()) == title))
    {
        return Err(io::Error::other(format!(
            "Update the “{}” workflow before removing or renaming this command.",
            workflow.title
        )));
    }
    Ok(())
}

fn list_error() -> io::Error {
    io::Error::other("The command list has an invalid entry.")
}

trait Record {
    fn patch(&self, table: &mut dyn TableLike, previous: Option<&Self>);
}

fn update_list<T: Record>(
    list: &mut Item,
    index: Option<usize>,
    previous: Option<&T>,
    next: Option<&T>,
) -> io::Result<()> {
    if let (Some(index), Some(next)) = (index, next) {
        let table = list.get_mut(index).and_then(Item::as_table_like_mut).ok_or_else(list_error)?;
        next.patch(table, previous);
        return Ok(());
    }
    match (list, index, next) {
        (Item::ArrayOfTables(tables), _, Some(next)) => {
            let mut table = Table::new();
            next.patch(&mut table, None);
            tables.push(table);
        }
        (Item::Value(Value::Array(array)), _, Some(next)) => {
            let mut table = InlineTable::new();
            next.patch(&mut table, None);
            array.push(table);
        }
        (Item::ArrayOfTables(tables), Some(index), None) => {
            tables.remove(index);
        }
        (Item::Value(Value::Array(array)), Some(index), None) => {
            array.remove(index);
        }
        _ => return Err(list_error()),
    }
    Ok(())
}

impl Record for Script {
    fn patch(&self, table: &mut dyn TableLike, previous: Option<&Self>) {
        let next = self;
        if previous.is_none_or(|old| old.title != next.title) {
            set(table, "title", next.title.as_str().into());
        }
        if previous.is_none_or(|old| old.executable != next.executable) {
            set(table, "executable", next.executable.to_string_lossy().into_owned().into());
        }
        if previous.is_none_or(|old| old.description != next.description) {
            set(table, "description", next.description.as_str().into());
        }
        if previous.is_none_or(|old| old.args != next.args) {
            set(table, "args", argument_array(&next.args).into());
        }
        if previous.map_or(!next.parameters.is_empty(), |old| old.parameters != next.parameters) {
            set(table, "parameters", argument_array(&next.parameters).into());
        }
        if previous.is_none_or(|old| old.confirm != next.confirm) {
            set(table, "confirm", next.confirm.into());
        }
    }
}

impl Record for Workflow {
    fn patch(&self, table: &mut dyn TableLike, previous: Option<&Self>) {
        if previous.is_none_or(|old| old.title != self.title) {
            set(table, "title", self.title.as_str().into());
        }
        if previous.is_none_or(|old| old.description != self.description) {
            set(table, "description", self.description.as_str().into());
        }
        if previous.is_none_or(|old| old.steps != self.steps) {
            set(table, "steps", argument_array(&self.steps).into());
        }
    }
}

fn set(table: &mut dyn TableLike, key: &str, mut value: Value) {
    if let Some(old) = table.get(key).and_then(Item::as_value) {
        if let (Some(previous), Some(next)) = (old.as_array(), value.as_array_mut()) {
            *next = retain_array_formatting(previous, next);
        } else {
            *value.decor_mut() = old.decor().clone();
        }
    }
    table.insert(key, Item::Value(value));
}

fn retain_array_formatting(previous: &Array, next: &Array) -> Array {
    let mut entries = HashMap::<&str, VecDeque<&Value>>::new();
    for (text, entry) in previous.iter().filter_map(|entry| Some((entry.as_str()?, entry))) {
        entries.entry(text).or_default().push_back(entry);
    }
    let mut formatted = Array::new();
    *formatted.decor_mut() = previous.decor().clone();
    formatted.set_trailing(previous.trailing().clone());
    formatted.set_trailing_comma(previous.trailing_comma() && !next.is_empty());
    for entry in next.iter() {
        if let Some(old) = entry.as_str().and_then(|text| entries.get_mut(text)).and_then(VecDeque::pop_front) {
            formatted.push_formatted(old.clone());
        } else {
            formatted.push(entry.clone());
        }
    }
    formatted
}

// The inode detects replacements, while ctime catches in-place edits, including those
// that preserve the modification time.
fn revision(path: &Path) -> io::Result<Option<(u64, u64, i64, i64)>> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(io::Error::other("Edit the linked launcher-scripts.toml file directly."))
        }
        Ok(meta) => Ok(Some((meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec()))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn changed_on_disk() -> io::Error {
    io::Error::other("The command list changed on disk. Reopen FerroSeek before saving.")
}

fn persist(path: &Path, source: Option<&str>, text: &str) -> io::Result<Option<String>> {
    let parent = path.parent().ok_or_else(|| io::Error::other("The command manifest needs a parent folder."))?;
    let directory = std::fs::File::open(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(text.as_bytes())?;
    temp.as_file().sync_all()?;
    if let Some(source) = source {
        let before = revision(path)?;
        if read_source(path)?.as_deref() != Some(source) || revision(path)? != before {
            return Err(changed_on_disk());
        }
        temp.persist(path).map_err(|error| error.error)?;
    } else {
        temp.persist_noclobber(path).map_err(|error| {
            if error.error.kind() == io::ErrorKind::AlreadyExists { changed_on_disk() } else { error.error }
        })?;
    }
    Ok(directory.sync_all().err().map(|error| {
        log::warn!("Could not sync command manifest directory: {error}");
        "Commands were written. Disk sync failed.".to_owned()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_round_trip_with_spaces_quotes_newlines_and_empty_entries() {
        let args = ["a b", "x\"y", "first\nsecond", "", "$HOME"].map(str::to_owned);
        assert_eq!(parse_arguments(&format_arguments(&args)).unwrap(), args);
        assert!(parse_arguments("").unwrap().is_empty());
        assert!(parse_arguments("[3]").is_err());
        assert!(parse_arguments("bash -c launch").is_err());
    }

    #[test]
    fn edit_preserves_manual_comments_and_rejects_stale_commands() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join("config.toml");
        let path = manifest_path(&settings);
        let source = "# Command catalogue\n[[command]]\ntitle = 'Check' # command title\nexecutable = '/synthetic/check'\n\n[[command]]\ntitle = 'Other'\nexecutable = '/synthetic/other'\n";
        std::fs::write(&path, source).unwrap();
        let original = Catalog::load(&settings).commands[0].clone();
        let next = Script { title: "Check version".into(), ..(*original).clone() };
        let catalog = change(&settings, Some(&original), Some(next)).unwrap().catalog;
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("# Command catalogue") && written.contains("# command title"));
        assert!(written.contains("title = 'Other'\nexecutable = '/synthetic/other'"));
        assert!(change(&settings, Some(&original), None).unwrap_err().contains("changed on disk"));
        let remaining = change(&settings, Some(&catalog.commands[0]), None).unwrap().catalog;
        assert_eq!(remaining.commands.len(), 1);
    }

    #[test]
    fn retains_comments_and_quoting_on_matching_array_entries() {
        let mut table = r#"
args = [
    # A flag already in use.
    '--summary',
    # Another occurrence of the same flag.
    "--summary",
    '--obsolete',
]
"#
        .parse::<DocumentMut>()
        .unwrap();
        let args = ["--summary", "--color", "--summary"].map(str::to_owned);
        set(&mut *table, "args", argument_array(&args).into());
        let written = table.to_string();
        assert!(written.contains("# A flag already in use.\n    '--summary'"));
        assert!(written.contains("# Another occurrence of the same flag.\n    \"--summary\""));
        assert!(!written.contains("--obsolete"));
        let source = format!("[[command]]\ntitle = 'Check'\nexecutable = '/synthetic/tool'\n{written}");
        assert_eq!(parse_manifest(&source).unwrap().command[0].args, args);
    }

    #[test]
    fn preserves_manual_writes_after_the_editor_reads_the_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("launcher-scripts.toml");
        for source in [None, Some("# Initial version\n")] {
            if let Some(source) = source {
                std::fs::write(&path, source).unwrap();
            }
            let manual = "# Saved manually\n";
            let result = edit(&path, |_, document| {
                std::fs::write(&path, manual)?;
                *document = "# Saved in FerroSeek\n".parse().unwrap();
                Ok(())
            });
            assert!(result.unwrap_err().to_string().contains("changed on disk"));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), manual);
        }
    }

    #[test]
    fn inline_command_lists_keep_their_shape_and_prevent_duplicate_titles() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join("config.toml");
        let path = manifest_path(&settings);
        std::fs::write(&path, "command = [] # local commands\n").unwrap();
        let script = Script {
            title: "Test report".into(),
            description: String::new(),
            executable: "/synthetic/report".into(),
            args: vec![],
            parameters: vec!["Project".into()],
            confirm: true,
        };
        let catalog = change(&settings, None, Some(script.clone())).unwrap().catalog;
        assert!(change(&settings, None, Some(script)).unwrap_err().contains("different title"));
        let updated = Script { confirm: false, ..(*catalog.commands[0]).clone() };
        let catalog = change(&settings, Some(&catalog.commands[0]), Some(updated)).unwrap().catalog;
        assert!(!catalog.commands[0].confirm);
        assert!(std::fs::read_to_string(path).unwrap().contains("# local commands"));
    }

    #[test]
    fn rejects_removing_a_command_used_by_a_workflow() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join("config.toml");
        let path = manifest_path(&settings);
        let source = r#"
[[command]]
title = "Build"
executable = "/synthetic/build"

[[workflow]]
title = "Prepare"
steps = ["BUILD", "Build"]
"#;
        std::fs::write(&path, source).unwrap();
        let catalog = Catalog::load(&settings);
        let command = &catalog.commands[0];
        assert!(change(&settings, Some(command), None).unwrap_err().contains("Prepare"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
    }

    #[test]
    fn changes_workflow_order_atomically_and_preserves_its_comment() {
        let temp = tempfile::tempdir().unwrap();
        let settings = temp.path().join("config.toml");
        let path = manifest_path(&settings);
        let source = r#"
[[command]]
title = "Build"
executable = "/synthetic/build"

[[command]]
title = "Check"
executable = "/synthetic/check"

[[workflow]]
title = "Prepare" # Keep workflow context.
steps = ["Build", "Check"]
"#;
        std::fs::write(&path, source).unwrap();
        let catalog = Catalog::load(&settings);
        let original = &catalog.workflows[0];
        let invalid = Workflow { steps: vec!["Missing".into(), "Build".into()], ..(**original).clone() };
        assert!(change_workflow(&settings, Some(original), Some(invalid)).unwrap_err().contains("Missing"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
        let updated = Workflow { steps: vec!["Check".into(), "Build".into()], ..(**original).clone() };
        let catalog = change_workflow(&settings, Some(original), Some(updated)).unwrap().catalog;
        assert_eq!(catalog.workflows[0].steps, ["Check", "Build"]);
        assert!(std::fs::read_to_string(&path).unwrap().contains("# Keep workflow context."));
        assert!(change_workflow(&settings, Some(original), None).unwrap_err().contains("changed on disk"));
        let catalog = change_workflow(&settings, Some(&catalog.workflows[0]), None).unwrap().catalog;
        assert!(catalog.workflows.is_empty());
    }
}
