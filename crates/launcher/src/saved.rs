// SPDX-License-Identifier: GPL-3.0-or-later
//! Atomic private storage for saved links and command aliases.

use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::links::{Link, LinkFields, normalize_keyword};
use crate::usage;

const MAX_LINKS: usize = 128;
const MAX_ALIASES: usize = 128;
const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasTarget {
    Application(String),
    Command(String),
}

impl AliasTarget {
    pub fn id(&self) -> &str {
        match self {
            Self::Application(id) | Self::Command(id) => id,
        }
    }

    fn tag(&self) -> &'static str {
        match self {
            Self::Application(_) => "app",
            Self::Command(_) => "command",
        }
    }

    fn parse(kind: &str, id: &str) -> io::Result<Self> {
        match kind {
            "app" => Ok(Self::Application(id.into())),
            "command" => Ok(Self::Command(id.into())),
            _ => Err(invalid_entry()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    pub target: AliasTarget,
    pub word: String,
}

impl Alias {
    pub fn new(target: AliasTarget, input: &str) -> Result<Self, String> {
        let word = normalize_keyword(input)?;
        if word.is_empty() {
            return Err("Enter an alias.".into());
        }
        let id = target.id();
        if id.is_empty() || id.len() > 4096 {
            return Err("This target cannot be saved as an alias.".into());
        }
        if id.chars().any(char::is_control) {
            return Err("This target has an invalid identifier.".into());
        }
        Ok(Self { target, word })
    }
}

#[derive(Debug, Clone, Default)]
pub struct Store {
    pub links: Vec<Link>,
    pub aliases: Vec<Alias>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeywordOwner {
    Link(usize),
    Alias(usize),
}

impl Store {
    pub fn path(settings: &Path) -> PathBuf {
        let path = settings.with_file_name("launcher-links");
        if path.is_absolute() { path } else { Path::new(".").join(path) }
    }

    pub fn load(path: &Path) -> Self {
        read(path).unwrap_or_else(|error| Self { error: Some(error.to_string()), ..Self::default() })
    }

    fn link_index(&self, previous: Option<&Link>) -> io::Result<Option<usize>> {
        previous
            .map(|old| {
                self.links.iter().position(|item| item == old).ok_or_else(|| {
                    io::Error::other("The link changed on disk. Reopen the launcher to load the latest links.")
                })
            })
            .transpose()
    }

    fn check_keyword(&self, word: &str, editing: Option<KeywordOwner>) -> io::Result<()> {
        if word.is_empty() {
            return Ok(());
        }
        let link = self.links.iter().position(|link| link.keyword == word).map(KeywordOwner::Link);
        let owner = link.or_else(|| self.aliases.iter().position(|alias| alias.word == word).map(KeywordOwner::Alias));
        if owner == editing {
            return Ok(());
        }
        match owner {
            Some(KeywordOwner::Link(_)) => Err(io::Error::other("A saved link already uses this keyword.")),
            Some(KeywordOwner::Alias(_)) => Err(io::Error::other("This keyword is already assigned to an alias.")),
            None => Ok(()),
        }
    }

    fn validate_link(&self, link: &Link, index: Option<usize>) -> io::Result<()> {
        if self.links.iter().enumerate().any(|(i, other)| index != Some(i) && other.has_same_name(link)) {
            return Err(io::Error::other("A saved link already uses this name."));
        }
        self.check_keyword(&link.keyword, index.map(KeywordOwner::Link))?;
        if index.is_none() && self.links.len() == MAX_LINKS {
            return Err(io::Error::other("The saved links limit is 128. Delete a link before adding another."));
        }
        Ok(())
    }

    fn apply_link(&mut self, previous: Option<&Link>, next: Option<Link>) -> io::Result<()> {
        let index = self.link_index(previous)?;
        if let Some(link) = &next {
            self.validate_link(link, index)?;
        }
        replace(&mut self.links, index, next)
    }

    fn validate_alias(&self, alias: &Alias, index: Option<usize>) -> io::Result<()> {
        self.check_keyword(&alias.word, index.map(KeywordOwner::Alias))?;
        if index.is_none() && self.aliases.len() == MAX_ALIASES {
            return Err(io::Error::other("The alias limit is 128. Remove an alias before adding another."));
        }
        Ok(())
    }

    fn apply_alias(&mut self, target: &AliasTarget, previous: Option<&str>, next: Option<Alias>) -> io::Result<()> {
        let index = self.aliases.iter().position(|alias| &alias.target == target);
        let stored = index.map(|index| self.aliases[index].word.as_str());
        if stored != previous {
            return Err(io::Error::other("This target's alias changed on disk. Reopen the launcher and try again."));
        }
        if let Some(alias) = &next {
            self.validate_alias(alias, index)?;
        }
        replace(&mut self.aliases, index, next)
    }
}

fn replace<T>(items: &mut Vec<T>, index: Option<usize>, next: Option<T>) -> io::Result<()> {
    match (index, next) {
        (Some(i), Some(entry)) => items[i] = entry,
        (Some(i), None) => {
            items.remove(i);
        }
        (None, Some(entry)) => items.push(entry),
        (None, None) => return Err(io::Error::other("Choose a saved entry to remove.")),
    }
    Ok(())
}

pub fn change_link(path: &Path, previous: Option<&Link>, next: Option<Link>) -> Result<Store, String> {
    change(path, |store| store.apply_link(previous, next))
}

pub fn change_alias(
    path: &Path,
    target: AliasTarget,
    previous: Option<&str>,
    word: Option<&str>,
) -> Result<Store, String> {
    let alias = word.map(|word| Alias::new(target.clone(), word)).transpose()?;
    change(path, |store| store.apply_alias(&target, previous, alias))
}

fn change(path: &Path, edit: impl FnOnce(&mut Store) -> io::Result<()>) -> Result<Store, String> {
    usage::with_lock(path, false, || {
        let mut store = read(path)?;
        edit(&mut store)?;
        write(path, &store)?;
        Ok(store)
    })
    .map_err(|error| {
        if error.kind() == io::ErrorKind::WouldBlock {
            "Saved links and aliases are busy. Try again.".into()
        } else {
            error.to_string()
        }
    })
}

fn read(path: &Path) -> io::Result<Store> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Store::default()),
        Err(e) => return Err(io::Error::other(format!("Cannot read saved links and aliases: {e}"))),
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("Saved links and aliases must be a regular file."));
    }
    let mut source = String::new();
    file.take(MAX_BYTES + 1).read_to_string(&mut source)?;
    if source.len() as u64 > MAX_BYTES {
        return Err(io::Error::other("The saved links and aliases file is too large."));
    }
    let mut store = Store::default();
    for line in source.lines() {
        read_line(&mut store, line)?;
    }
    Ok(store)
}

fn read_line(store: &mut Store, line: &str) -> io::Result<()> {
    let mut fields = line.splitn(4, '\t');
    match (fields.next(), fields.next(), fields.next(), fields.next()) {
        (Some(name), Some(address), Some(keyword), None) => {
            let link = Link::new(LinkFields { name, address, keyword }).map_err(io::Error::other)?;
            store.apply_link(None, Some(link))
        }
        (Some("alias"), Some(kind), Some(id), Some(word)) => {
            let target = AliasTarget::parse(kind, id)?;
            let alias = Alias::new(target.clone(), word).map_err(io::Error::other)?;
            store.apply_alias(&target, None, Some(alias))
        }
        _ => Err(invalid_entry()),
    }
}

fn invalid_entry() -> io::Error {
    io::Error::other("The saved links and aliases file has an invalid entry.")
}

fn write(path: &Path, store: &Store) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| io::Error::other("Saved entries need a parent folder."))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    for link in &store.links {
        writeln!(tmp, "{}\t{}\t{}", link.name, link.address, link.keyword)?;
    }
    for alias in &store.aliases {
        writeln!(tmp, "alias\t{}\t{}\t{}", alias.target.tag(), alias.target.id(), alias.word)?;
    }
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    std::fs::File::open(parent).and_then(|dir| dir.sync_all()).map_err(|e| {
        io::Error::other(format!(
            "Entries were written but durability could not be confirmed. Reopen the launcher: {e}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_merge_existing_links_and_aliases_and_refuse_stale_replacements() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("launcher-links");
        let text = "One\thttps://example.net/one\tone\n";
        std::fs::write(&path, text).unwrap();
        let original =
            Link::new(LinkFields { name: "One", address: "https://example.net/one", keyword: "one" }).unwrap();
        let edited =
            Link::new(LinkFields { name: "Renamed", address: "https://example.net/new", keyword: "one" }).unwrap();
        let target = AliasTarget::Application("org.example.One.desktop".into());
        change_alias(&path, target.clone(), None, Some("rUn")).unwrap();
        change_link(&path, Some(&original), Some(edited.clone())).unwrap();
        assert!(change_link(&path, Some(&original), None).is_err());
        assert!(change_alias(&path, target.clone(), None, Some("other")).is_err());
        let loaded = Store::load(&path);
        assert_eq!(loaded.links.as_slice(), std::slice::from_ref(&edited));
        assert_eq!(loaded.aliases, [Alias::new(target.clone(), "run").unwrap()]);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let saved = change_alias(&path, target, Some("run"), None).unwrap();
        assert!(saved.aliases.is_empty());
        assert_eq!(saved.links, [edited]);
    }

    #[test]
    fn aliases_and_saved_link_keywords_share_one_namespace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("launcher-links");
        let link = Link::new(LinkFields { name: "Example", address: "https://example.net/", keyword: "go" }).unwrap();
        change_link(&path, None, Some(link.clone())).unwrap();
        let target = AliasTarget::Command("search-status".into());
        assert!(change_alias(&path, target.clone(), None, Some("GO")).is_err());
        change_link(&path, Some(&link), None).unwrap();
        change_alias(&path, target, None, Some("go")).unwrap();
        assert!(change_link(&path, None, Some(link)).is_err());
    }
}
