// SPDX-License-Identifier: GPL-3.0-or-later
//! User-defined launcher destinations and their private local storage.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use url::Url;

use crate::{matcher, usage};

const MAX_LINKS: usize = 128;
const MAX_ADDRESS: usize = 2048;
const MAX_TERMS: usize = 2048;
const MAX_BYTES: u64 = 384 * 1024;
const QUERY: &str = "{query}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub name: String,
    pub address: String,
    pub keyword: String,
    validated: Destination,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Destination {
    Web(Url),
    Template { host: String },
    Indexed(Url),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Open {
    Web(String),
    Indexed(String),
    Prompt(String),
}

impl Link {
    pub fn new(name: &str, address: &str, keyword: &str) -> Result<Self, String> {
        let name = field(name, 120, "Name")?;
        let address = field(address, MAX_ADDRESS, "Destination")?;
        let keyword = field(keyword, 32, "Keyword")?.to_ascii_lowercase();
        if name.is_empty() {
            return Err("Enter a name.".into());
        }
        if !keyword.bytes().all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_')) {
            return Err("Use letters, digits, hyphens or underscores in the keyword.".into());
        }
        let validated = Destination::parse(address, &keyword)?;
        Ok(Self { name: name.into(), address: address.into(), keyword, validated })
    }

    pub fn is_indexed(&self) -> bool {
        matches!(self.validated, Destination::Indexed(_))
    }

    pub fn subtitle(&self) -> String {
        let destination = match &self.validated {
            Destination::Indexed(_) => self.address.as_str(),
            Destination::Template { host } => host.as_str(),
            Destination::Web(url) => url.host_str().unwrap_or_default(),
        };
        if self.keyword.is_empty() { destination.into() } else { format!("{destination} · {}", self.keyword) }
    }

    pub fn open(&self, terms: &str) -> Result<Open, String> {
        match &self.validated {
            Destination::Web(url) => Ok(Open::Web(url.as_str().into())),
            Destination::Indexed(url) => Ok(Open::Indexed(url.as_str().into())),
            Destination::Template { .. } if terms.is_empty() => Ok(Open::Prompt(format!("{} ", self.keyword))),
            Destination::Template { .. } => {
                if terms.len() > MAX_TERMS {
                    return Err("Search terms are too long.".into());
                }
                let encoded: String = url::form_urlencoded::byte_serialize(terms.as_bytes()).collect();
                let url = web_url(&self.address.replacen(QUERY, &encoded, 1))?;
                Ok(Open::Web(url.into()))
            }
        }
    }

    fn conflicts_with(&self, other: &Self) -> bool {
        matcher::fold(&self.name) == matcher::fold(&other.name)
            || (!self.keyword.is_empty() && self.keyword == other.keyword)
    }
}

fn field<'a>(value: &'a str, max: usize, name: &str) -> Result<&'a str, String> {
    let trimmed = value.trim();
    if trimmed.len() > max || value.chars().any(char::is_control) {
        return Err(format!("{name} must fit on one line and use at most {max} bytes."));
    }
    Ok(trimmed)
}

impl Destination {
    fn parse(address: &str, keyword: &str) -> Result<Self, String> {
        if address.starts_with('/') || address.starts_with("~/") {
            let path = ferroseek_config::normalize(&ferroseek_config::expand(address));
            return Url::from_file_path(path).map(Self::Indexed).map_err(|_| "Enter an absolute indexed path.".into());
        }
        let Some((before, after)) = address.split_once(QUERY) else {
            return web_url(address).map(Self::Web);
        };
        if keyword.is_empty() {
            return Err("A web search needs a keyword.".into());
        }
        let in_value = before.split_once('?').is_some_and(|(_, params)| {
            !params.contains('#') && params.rsplit('&').next().is_some_and(|param| param.contains('='))
        });
        if !in_value || after.contains(QUERY) {
            return Err("Use {query} once in a URL query value, such as ?q={query}.".into());
        }
        let parsed = web_url(&address.replacen(QUERY, "search", 1))?;
        Ok(Self::Template { host: parsed.host_str().unwrap_or_default().into() })
    }
}

fn web_url(value: &str) -> Result<Url, String> {
    let scheme = value.split_once("://").map(|(scheme, _)| scheme);
    if !scheme.is_some_and(|s| s.eq_ignore_ascii_case("https") || s.eq_ignore_ascii_case("http")) {
        return Err("Use an HTTPS or HTTP address, an absolute path, or a path starting with ~/.".into());
    }
    let url = Url::parse(value).map_err(|_| "Enter a valid web address.")?;
    if !url.has_host() {
        return Err("Enter a web address with a host.".into());
    }
    if url.authority().contains('@') {
        return Err("Remove embedded credentials from the web address.".into());
    }
    Ok(url)
}

fn keyword_match(link: &Link, first: &str, terms: &str) -> bool {
    matches!(
        (link.keyword.eq_ignore_ascii_case(first), terms.is_empty(), &link.validated),
        (true, true, _) | (true, _, Destination::Template { .. })
    )
}

pub fn matching<'a>(links: &'a [Link], query: &'a str, all: bool) -> Vec<(&'a Link, &'a str)> {
    let query = query.trim();
    if query.is_empty() {
        return if all { Vec::new() } else { links.iter().map(|link| (link, "")).collect() };
    }
    let split = query.find(char::is_whitespace).unwrap_or(query.len());
    let (first, terms) = query.split_at(split);
    let terms = terms.trim();
    let tokens = matcher::tokens(query);
    let minimum = match (all, query.chars().take(2).count()) {
        (true, 1) => u32::MAX,
        (true, _) => 600,
        _ => 0,
    };
    let mut ranked: Vec<_> = links
        .iter()
        .filter_map(|link| {
            if keyword_match(link, first, terms) {
                return Some((link, terms, matcher::EXACT + 1));
            }
            let title = matcher::fold(&link.name);
            let score = matcher::score(&tokens, &[(title.as_str(), 100), (&link.keyword, 80)])?;
            (score >= minimum).then_some((link, "", score))
        })
        .collect();
    ranked.sort_by_key(|&(_, _, score)| std::cmp::Reverse(score));
    ranked.into_iter().map(|(link, terms, _)| (link, terms)).collect()
}

#[derive(Debug, Default)]
pub struct Store {
    pub items: Vec<Link>,
    pub error: Option<String>,
}

impl Store {
    pub fn path(settings: &Path) -> PathBuf {
        let path = settings.with_file_name("launcher-links");
        if path.is_absolute() { path } else { Path::new(".").join(path) }
    }

    pub fn load(path: &Path) -> Self {
        match read(path) {
            Ok(items) => Self { items, error: None },
            Err(error) => Self { items: Vec::new(), error: Some(error.to_string()) },
        }
    }
}

pub fn change(path: &Path, previous: Option<&Link>, next: Option<Link>) -> Result<Vec<Link>, String> {
    usage::with_lock(path, false, || {
        let mut items = read(path)?;
        apply(&mut items, previous, next)?;
        write(path, &items)?;
        Ok(items)
    })
    .map_err(|error| {
        if error.kind() == io::ErrorKind::WouldBlock { "Links are busy. Try again.".into() } else { error.to_string() }
    })
}

fn apply(items: &mut Vec<Link>, previous: Option<&Link>, next: Option<Link>) -> io::Result<()> {
    let index = previous
        .map(|old| {
            items.iter().position(|item| item == old).ok_or_else(|| {
                io::Error::other("The link changed on disk. Reopen the launcher to load the latest links.")
            })
        })
        .transpose()?;
    if let Some(link) = &next {
        if items.iter().enumerate().any(|(i, other)| index != Some(i) && other.conflicts_with(link)) {
            return Err(io::Error::other("This name or keyword is already in use."));
        }
        if index.is_none() && items.len() == MAX_LINKS {
            return Err(io::Error::other("The saved links limit is 128. Delete a link before adding another."));
        }
    }
    match (index, next) {
        (Some(i), Some(link)) => items[i] = link,
        (Some(i), None) => {
            items.remove(i);
        }
        (None, Some(link)) => items.push(link),
        (None, None) => return Err(io::Error::other("Choose a link to delete.")),
    }
    Ok(())
}

fn read(path: &Path) -> io::Result<Vec<Link>> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io::Error::other(format!("Cannot read saved links: {e}"))),
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("Saved links must be a regular file."));
    }
    let mut source = String::new();
    file.take(MAX_BYTES + 1).read_to_string(&mut source)?;
    if source.len() as u64 > MAX_BYTES {
        return Err(io::Error::other("The saved links file is too large."));
    }
    let mut items = Vec::new();
    for line in source.lines() {
        let mut parts = line.splitn(3, '\t');
        let columns = (parts.next(), parts.next(), parts.next());
        let (Some(name), Some(address), Some(keyword)) = columns else {
            return Err(io::Error::other("The saved links file has an invalid entry."));
        };
        let link = Link::new(name, address, keyword).map_err(io::Error::other)?;
        apply(&mut items, None, Some(link))?;
    }
    Ok(items)
}

fn write(path: &Path, links: &[Link]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| io::Error::other("Saved links need a parent folder."))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    for link in links {
        writeln!(tmp, "{}\t{}\t{}", link.name, link.address, link.keyword)?;
    }
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    std::fs::File::open(parent)?.sync_all().map_err(|e| {
        io::Error::other(format!("Links were written but durability could not be confirmed. Reopen the launcher: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_terms_are_only_data_in_the_web_search_parameter() {
        let link = Link::new("Search", "https://example.net/find?lang=en&q={query}#results", "Web").unwrap();
        assert_eq!(link.open(""), Ok(Open::Prompt("web ".into())));
        assert_eq!(
            link.open("caffè &x=1 + %"),
            Ok(Open::Web("https://example.net/find?lang=en&q=caff%C3%A8+%26x%3D1+%2B+%25#results".into()))
        );
        assert_eq!(matching(std::slice::from_ref(&link), "WEB  hello world", true), [(&link, "hello world")]);
    }

    #[test]
    fn validates_supported_addresses_without_opening_them() {
        for address in [
            "javascript:alert(1)",
            "https://user@example.net",
            "https://{query}.example.net",
            "https://example.net/?{query}=ignored",
            "https://example.net/?q={query}&x={query}",
            "https://example.net/\nignored",
            "relative/path",
        ] {
            assert!(Link::new("Example", address, "web").is_err(), "{address}");
        }
        let link = Link::new("Files", "/synthetic/Figures #2", "fig").unwrap();
        assert_eq!(link.open(""), Ok(Open::Indexed("file:///synthetic/Figures%20%232".into())));
    }

    #[test]
    fn edits_merge_with_other_links_and_refuse_stale_replacements() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("links");
        let first = Link::new("One", "https://example.net/one", "one").unwrap();
        let second = Link::new("Two", "https://example.net/two", "two").unwrap();
        let edited = Link::new("Renamed", "https://example.net/new", "one").unwrap();
        change(&path, None, Some(first.clone())).unwrap();
        change(&path, None, Some(second.clone())).unwrap();
        change(&path, Some(&first), Some(edited.clone())).unwrap();
        assert!(change(&path, Some(&first), None).is_err());
        assert!(change(&path, None, Some(edited.clone())).is_err());
        assert_eq!(Store::load(&path).items, [edited.clone(), second.clone()]);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(change(&path, Some(&edited), None).unwrap(), [second]);
    }
}
