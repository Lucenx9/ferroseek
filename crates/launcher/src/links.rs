// SPDX-License-Identifier: GPL-3.0-or-later
//! User-defined launcher destinations and argument matching.

use url::Url;

use crate::matcher;

const MAX_ADDRESS: usize = 2048;
const MAX_TERMS: usize = 2048;
pub(crate) const QUERY: &str = "{query}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub(crate) id: String,
    pub name: String,
    pub address: String,
    pub keyword: String,
    validated: Destination,
    fallback: bool,
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

pub struct LinkFields<'a> {
    pub name: &'a str,
    pub address: &'a str,
    pub keyword: &'a str,
}

impl Link {
    pub fn new(LinkFields { name, address, keyword }: LinkFields<'_>) -> Result<Self, String> {
        let name = field(name, 120).ok_or("Use a single-line name of at most 120 bytes.")?;
        let address = field(address, MAX_ADDRESS).ok_or("Use a one-line destination of at most 2048 bytes.")?;
        let keyword = normalize_keyword(keyword)?;
        if name.is_empty() {
            return Err("Enter a name.".into());
        }
        let validated = Destination::parse(address, !keyword.is_empty())?;
        Ok(Self {
            id: format!("legacy:{name}"),
            name: name.into(),
            address: address.into(),
            keyword,
            validated,
            fallback: false,
        })
    }

    pub fn with_fallback(mut self, enabled: bool) -> Result<Self, String> {
        if enabled && !matches!(self.validated, Destination::Template { .. }) {
            return Err(
                "A fallback needs a web search URL with {query}. Turn off the fallback to save this link.".into()
            );
        }
        self.fallback = enabled;
        Ok(self)
    }

    pub fn uses_fallback(&self) -> bool {
        self.fallback
    }

    pub fn fallback_host(&self, terms: &str) -> Option<&str> {
        match &self.validated {
            Destination::Template { host } if self.fallback && (1..=MAX_TERMS).contains(&terms.len()) => Some(host),
            _ => None,
        }
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
}

pub(crate) fn normalize_keyword(input: &str) -> Result<String, String> {
    let keyword = field(input, 32).ok_or("Use a single-line keyword of at most 32 bytes.")?.to_ascii_lowercase();
    if !keyword.bytes().all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_')) {
        return Err("Use letters, digits, hyphens or underscores in the keyword.".into());
    }
    Ok(keyword)
}

fn field(value: &str, max: usize) -> Option<&str> {
    let trimmed = value.trim();
    (trimmed.len() <= max && !value.chars().any(char::is_control)).then_some(trimmed)
}

impl Destination {
    fn parse(address: &str, has_keyword: bool) -> Result<Self, String> {
        if address.starts_with('/') || address.starts_with("~/") {
            let path = ferroseek_config::normalize(&ferroseek_config::expand(address));
            return Url::from_file_path(path).map(Self::Indexed).map_err(|_| "Enter an absolute indexed path.".into());
        }
        let Some((before, after)) = address.split_once(QUERY) else {
            return web_url(address).map(Self::Web);
        };
        if !has_keyword {
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

fn keyword_match(link: &Link, first: &str, has_terms: bool) -> bool {
    matches!(
        (link.keyword.eq_ignore_ascii_case(first), has_terms, &link.validated),
        (true, false, _) | (true, _, Destination::Template { .. })
    )
}

pub fn matching<'a>(links: &'a [Link], query: &'a str, all: bool) -> Vec<(&'a Link, &'a str, u32)> {
    let query = query.trim();
    if query.is_empty() {
        return if all { Vec::new() } else { links.iter().map(|link| (link, "", 0)).collect() };
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
    links
        .iter()
        .filter_map(|link| {
            if keyword_match(link, first, !terms.is_empty()) {
                return Some((link, terms, matcher::EXACT + 1));
            }
            let title = matcher::fold(&link.name);
            let score = matcher::score(&tokens, &[(title.as_str(), 100), (&link.keyword, 80)])?;
            (score >= minimum).then_some((link, "", score))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_terms_are_only_data_in_the_web_search_parameter() {
        let link = Link::new(LinkFields {
            name: "Search",
            address: "https://example.net/find?lang=en&q={query}#results",
            keyword: "Web",
        })
        .unwrap();
        assert_eq!(link.open(""), Ok(Open::Prompt("web ".into())));
        assert_eq!(
            link.open("caffè &x=1 + %"),
            Ok(Open::Web("https://example.net/find?lang=en&q=caff%C3%A8+%26x%3D1+%2B+%25#results".into()))
        );
        assert_eq!(
            matching(std::slice::from_ref(&link), "WEB  hello world", true),
            [(&link, "hello world", matcher::EXACT + 1)]
        );
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
            assert!(Link::new(LinkFields { name: "Example", address, keyword: "web" }).is_err(), "{address}");
        }
        let link = Link::new(LinkFields { name: "Files", address: "/synthetic/Figures #2", keyword: "fig" }).unwrap();
        assert_eq!(link.open(""), Ok(Open::Indexed("file:///synthetic/Figures%20%232".into())));
    }
}
