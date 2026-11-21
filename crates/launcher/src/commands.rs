// SPDX-License-Identifier: GPL-3.0-or-later
//! Searchable destinations implemented by the launcher itself.

use crate::matcher;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Settings,
    Appearance,
    Layout,
    SearchFolders,
    SearchStatus,
    CreateLink,
}

const ALL: [Builtin; 6] = [
    Builtin::Settings,
    Builtin::Appearance,
    Builtin::Layout,
    Builtin::SearchFolders,
    Builtin::SearchStatus,
    Builtin::CreateLink,
];

impl Builtin {
    pub fn title(self) -> &'static str {
        match self {
            Self::Settings => "Settings",
            Self::Appearance => "Appearance",
            Self::Layout => "Layout",
            Self::SearchFolders => "Search folders",
            Self::SearchStatus => "Search status",
            Self::CreateLink => "Create link",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Settings => "Open launcher settings",
            Self::Appearance => "Change the theme and surface",
            Self::Layout => "Choose window width and position",
            Self::SearchFolders => "Choose locations to index",
            Self::SearchStatus => "Check indexing progress",
            Self::CreateLink => "Save a website, indexed path or web search",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Settings => "preferences-system",
            Self::Appearance => "preferences-desktop-theme",
            Self::Layout => "preferences-system-windows",
            Self::SearchFolders => "folder-saved-search",
            Self::SearchStatus => "dialog-information",
            Self::CreateLink => "insert-link",
        }
    }

    fn keywords(self) -> &'static str {
        match self {
            Self::Settings => "preferences configuration",
            Self::Appearance => "theme colors dark light glass solid",
            Self::Layout => "window width position compact centered",
            Self::SearchFolders => "directories locations roots",
            Self::SearchStatus => "indexing diagnostics progress",
            Self::CreateLink => "new saved shortcut quicklink bookmark",
        }
    }

    fn score(self, tokens: &[String]) -> Option<u32> {
        let title = matcher::fold(self.title());
        if tokens.iter().map(String::as_str).eq(title.split_whitespace()) {
            return Some(matcher::EXACT);
        }
        matcher::score(tokens, &[(title.as_str(), 100), (self.keywords(), 80)])
    }
}

/// In All, show strong matches for two or more characters. The dedicated
/// scope lists the entire catalog with an empty query and allows fuzzy matches.
pub fn matching(query: &str, commands_only: bool) -> Vec<Builtin> {
    if query.trim().is_empty() {
        return if commands_only { ALL.to_vec() } else { Vec::new() };
    }
    if !commands_only && query.trim().chars().take(2).count() < 2 {
        return Vec::new();
    }

    let tokens = matcher::tokens(query);
    let mut ranked: Vec<_> = ALL
        .into_iter()
        .filter_map(|command| {
            let score = command.score(&tokens)?;
            (commands_only || score >= 600).then_some((command, score))
        })
        .collect();
    ranked.sort_by_key(|&(_, score)| std::cmp::Reverse(score));
    ranked.into_iter().map(|(command, _)| command).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_search_keeps_only_strong_command_matches() {
        for (query, expected) in [
            ("", &[][..]),
            ("s", &[]),
            ("report", &[]),
            ("  SEARCH FOLDERS ", &[Builtin::SearchFolders]),
            ("dark theme", &[Builtin::Appearance]),
            ("width", &[Builtin::Layout]),
        ] {
            assert_eq!(matching(query, false), expected, "{query}");
        }
    }

    #[test]
    fn dedicated_scope_lists_every_command_and_accepts_fuzzy_input() {
        assert_eq!(matching("", true), ALL);
        assert_eq!(matching("srch stat", true).first(), Some(&Builtin::SearchStatus));
        assert!(matching("zebra cake", true).is_empty());
    }
}
