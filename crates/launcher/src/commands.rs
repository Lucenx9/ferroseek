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
    Shortcuts,
    CreateSnippet,
    ClipboardHistory,
    SwitchWindows,
}

pub const ALL: [Builtin; 10] = [
    Builtin::Settings,
    Builtin::Appearance,
    Builtin::Layout,
    Builtin::SearchFolders,
    Builtin::SearchStatus,
    Builtin::CreateLink,
    Builtin::Shortcuts,
    Builtin::CreateSnippet,
    Builtin::ClipboardHistory,
    Builtin::SwitchWindows,
];

struct Metadata {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    icon: &'static str,
    keywords: &'static str,
}

const SWITCH_WINDOWS: Metadata = Metadata {
    id: "switch-windows",
    title: "Switch windows",
    description: "Find and focus an open window",
    icon: "focus-windows",
    keywords: "open windows find running apps activate focus switcher",
};

impl Builtin {
    fn metadata(self) -> &'static Metadata {
        match self {
            Self::Settings => &Metadata {
                id: "settings",
                title: "Settings",
                description: "Open launcher settings",
                icon: "preferences-system",
                keywords: "preferences configuration",
            },
            Self::Appearance => &Metadata {
                id: "appearance",
                title: "Appearance",
                description: "Change the theme and surface",
                icon: "preferences-desktop-theme",
                keywords: "theme colors dark light glass solid",
            },
            Self::Layout => &Metadata {
                id: "layout",
                title: "Layout",
                description: "Choose window width and position",
                icon: "preferences-system-windows",
                keywords: "window width position compact centered",
            },
            Self::SearchFolders => &Metadata {
                id: "search-folders",
                title: "Search folders",
                description: "Choose locations to index",
                icon: "folder-saved-search",
                keywords: "directories locations roots",
            },
            Self::SearchStatus => &Metadata {
                id: "search-status",
                title: "Search status",
                description: "Check indexing progress",
                icon: "dialog-information",
                keywords: "indexing diagnostics progress",
            },
            Self::Shortcuts => &Metadata {
                id: "shortcuts",
                title: "Shortcuts",
                description: "Change launcher keyboard bindings",
                icon: "preferences-desktop-keyboard",
                keywords: "keys keyboard shortcuts bindings hotkeys",
            },
            Self::ClipboardHistory => &Metadata {
                id: "clipboard-history",
                title: "Clipboard history",
                description: "Find and reuse copied text",
                icon: "edit-paste",
                keywords: "copies recent clipboard history paste",
            },
            Self::SwitchWindows => &SWITCH_WINDOWS,
            Self::CreateSnippet => &Metadata {
                id: "create-snippet",
                title: "Create snippet",
                description: "Save text to copy again",
                icon: "edit-paste",
                keywords: "new text template snippets reusable clipboard",
            },
            Self::CreateLink => &Metadata {
                id: "create-link",
                title: "Create link",
                description: "Save a website, indexed path or web search",
                icon: "insert-link",
                keywords: "new saved shortcut quicklink bookmark",
            },
        }
    }

    pub fn id(self) -> &'static str {
        self.metadata().id
    }

    pub fn from_id(id: &str) -> Option<Self> {
        ALL.into_iter().find(|command| command.id() == id)
    }

    pub fn title(self) -> &'static str {
        self.metadata().title
    }

    pub fn description(self) -> &'static str {
        self.metadata().description
    }

    pub fn icon(self) -> &'static str {
        self.metadata().icon
    }

    fn score(self, tokens: &[String]) -> Option<u32> {
        let title = matcher::fold(self.title());
        if tokens.iter().map(String::as_str).eq(title.split_whitespace()) {
            return Some(matcher::EXACT);
        }
        matcher::score(tokens, &[(title.as_str(), 100), (self.metadata().keywords, 80)])
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
