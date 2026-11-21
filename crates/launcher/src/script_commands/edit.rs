// SPDX-License-Identifier: GPL-3.0-or-later
//! Comment-preserving writes to the local command manifest.

use std::io::{self, Write};
use std::path::Path;

use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value};

use super::{Catalog, Script, ensure_size, manifest_path, parse_manifest, read_source};
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

pub fn change(settings: &Path, previous: Option<&Script>, next: Option<Script>) -> Result<Catalog, String> {
    let path = manifest_path(settings);
    usage::with_lock(&path, false, || edit(&path, previous, next)).map_err(|error| match error.kind() {
        io::ErrorKind::WouldBlock => "Local commands are busy. Try again.".into(),
        _ => error.to_string(),
    })
}

fn edit(path: &Path, previous: Option<&Script>, next: Option<Script>) -> io::Result<Catalog> {
    check_writable(path)?;
    let source = read_source(path)?;
    let original = source.as_deref().unwrap_or("");
    let commands = parse_manifest(original)?;
    let index = previous
        .map(|old| {
            commands
                .iter()
                .position(|entry| entry == old)
                .ok_or_else(|| io::Error::other("This command changed on disk. Reopen FerroSeek to load it."))
        })
        .transpose()?;

    let mut document = original.parse::<DocumentMut>().map_err(|error| io::Error::other(error.message().to_owned()))?;
    let list = document.as_table_mut().entry("command").or_insert(Item::ArrayOfTables(ArrayOfTables::new()));
    update_list(list, index, previous, next.as_ref())?;
    let text = document.to_string();
    ensure_size(&text)?;
    let commands = parse_manifest(&text)?;
    if text != original {
        persist(path, source.as_deref(), &text)?;
    }
    Ok(Catalog::from_commands(commands))
}

fn list_error() -> io::Error {
    io::Error::other("The command list has an invalid entry.")
}

fn update_list(
    list: &mut Item,
    index: Option<usize>,
    previous: Option<&Script>,
    next: Option<&Script>,
) -> io::Result<()> {
    if let (Some(index), Some(next)) = (index, next) {
        let table = list.get_mut(index).and_then(Item::as_table_like_mut).ok_or_else(list_error)?;
        update_table(table, previous, next);
        return Ok(());
    }
    match (list, index, next) {
        (Item::ArrayOfTables(tables), _, Some(next)) => {
            let mut table = Table::new();
            update_table(&mut table, None, next);
            tables.push(table);
        }
        (Item::Value(Value::Array(array)), _, Some(next)) => {
            let mut table = InlineTable::new();
            update_table(&mut table, None, next);
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

fn update_table(table: &mut dyn TableLike, previous: Option<&Script>, next: &Script) {
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
    if previous.is_none_or(|old| old.confirm != next.confirm) {
        set(table, "confirm", next.confirm.into());
    }
}

fn set(table: &mut dyn TableLike, key: &str, mut value: Value) {
    if let Some(old) = table.get(key).and_then(Item::as_value) {
        *value.decor_mut() = old.decor().clone();
    }
    table.insert(key, Item::Value(value));
}

fn check_writable(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(io::Error::other("Edit the linked launcher-scripts.toml file directly."))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn persist(path: &Path, source: Option<&str>, text: &str) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| io::Error::other("The command manifest needs a parent folder."))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(text.as_bytes())?;
    temp.as_file().sync_all()?;
    check_writable(path)?;
    if read_source(path)?.as_deref() != source {
        return Err(io::Error::other("The command list changed on disk. Reopen FerroSeek before saving."));
    }
    temp.persist(path).map_err(|e| e.error)?;
    std::fs::File::open(parent).and_then(|dir| dir.sync_all()).map_err(|e| {
        io::Error::other(format!("Commands were written but durability could not be confirmed. Reopen FerroSeek. {e}"))
    })
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
        let catalog = change(&settings, Some(&original), Some(next)).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("# Command catalogue") && written.contains("# command title"));
        assert!(written.contains("title = 'Other'\nexecutable = '/synthetic/other'"));
        assert!(change(&settings, Some(&original), None).unwrap_err().contains("changed on disk"));
        let remaining = change(&settings, Some(&catalog.commands[0]), None).unwrap();
        assert_eq!(remaining.commands.len(), 1);
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
            confirm: true,
        };
        let catalog = change(&settings, None, Some(script.clone())).unwrap();
        assert!(change(&settings, None, Some(script)).unwrap_err().contains("different title"));
        let updated = Script { confirm: false, ..(*catalog.commands[0]).clone() };
        let catalog = change(&settings, Some(&catalog.commands[0]), Some(updated)).unwrap();
        assert!(!catalog.commands[0].confirm);
        assert!(std::fs::read_to_string(path).unwrap().contains("# local commands"));
    }
}
