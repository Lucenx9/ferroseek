// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded text saved explicitly for reuse.

use std::sync::Arc;

use crate::{links::normalize_keyword, matcher};

pub const MAX_BODY_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    pub(crate) id: String,
    pub name: String,
    pub keyword: String,
    pub body: Arc<str>,
}

pub struct Fields<'a> {
    pub name: &'a str,
    pub keyword: &'a str,
    pub body: &'a str,
}

impl Snippet {
    pub fn new(Fields { name, keyword, body }: Fields<'_>) -> Result<Self, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Enter a name.".into());
        }
        if name.len() > 120 || name.chars().any(char::is_control) {
            return Err("Use a single-line name of at most 120 bytes.".into());
        }
        if body.trim().is_empty() {
            return Err("Enter text to save.".into());
        }
        if body.len() > MAX_BODY_BYTES {
            return Err("Use at most 8 KiB of text.".into());
        }
        if body.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t' | '\r')) {
            return Err("Remove control characters from the text.".into());
        }
        Ok(Self {
            id: format!("legacy:{name}"),
            name: name.into(),
            keyword: normalize_keyword(keyword)?,
            body: body.into(),
        })
    }

    pub fn subtitle(&self) -> String {
        let lines = self.body.bytes().filter(|&byte| byte == b'\n').count() + 1;
        match (&*self.keyword, lines) {
            ("", 1) => "1 line".into(),
            ("", _) => format!("{lines} lines"),
            (keyword, 1) => format!("{keyword} · 1 line"),
            (keyword, _) => format!("{keyword} · {lines} lines"),
        }
    }
}

pub fn matching<'a>(snippets: &'a [Snippet], query: &str, all: bool) -> Vec<(&'a Snippet, u32)> {
    let query = query.trim();
    if query.is_empty() {
        return if all { Vec::new() } else { snippets.iter().map(|snippet| (snippet, 0)).collect() };
    }
    let tokens = matcher::tokens(query);
    let enough_chars = query.chars().take(2).count() == 2;
    snippets
        .iter()
        .filter_map(|snippet| {
            if snippet.keyword.eq_ignore_ascii_case(query) {
                return Some((snippet, matcher::EXACT + 1));
            }
            let name = matcher::fold(&snippet.name);
            let score = matcher::score(&tokens, &[(name.as_str(), 100), (&snippet.keyword, 80)])?;
            let strong = enough_chars && score >= 600;
            (!all || strong).then_some((snippet, score))
        })
        .collect()
}
