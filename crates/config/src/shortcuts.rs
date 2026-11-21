// SPDX-License-Identifier: GPL-3.0-or-later
//! Shortcuts that apply inside the launcher window.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Actions,
    Settings,
    Appearance,
    SearchFolders,
    SearchStatus,
}

impl Action {
    pub const ALL: [Self; 5] =
        [Self::Actions, Self::Settings, Self::Appearance, Self::SearchFolders, Self::SearchStatus];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Actions => "Actions",
            Self::Settings => "Settings",
            Self::Appearance => "Appearance",
            Self::SearchFolders => "Search folders",
            Self::SearchStatus => "Search status",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Chord {
    key: char,
    shift: bool,
}

impl Chord {
    pub fn new(key: char, shift: bool) -> Result<Self, &'static str> {
        let key = key.to_ascii_uppercase();
        let supported =
            if shift { (key.is_ascii_uppercase() && key != 'C') || key == ',' } else { matches!(key, 'I' | 'K' | ',') };
        supported
            .then_some(Self { key, shift })
            .ok_or("Use Ctrl+Shift with a letter or comma. Ctrl+Shift+C is reserved.")
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shift = if self.shift { "Shift+" } else { "" };
        write!(output, "Ctrl+{shift}{}", self.key)
    }
}

impl FromStr for Chord {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let key = value.strip_prefix("Ctrl+").ok_or("Start the shortcut with Ctrl+.")?;
        let shift = key.strip_prefix("Shift+");
        let mut chars = shift.unwrap_or(key).chars();
        match (chars.next(), chars.next()) {
            (Some(key), None) => Self::new(key, shift.is_some()),
            _ => Err("End the shortcut with one letter or a comma."),
        }
    }
}

impl TryFrom<String> for Chord {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<Chord> for String {
    fn from(value: Chord) -> Self {
        value.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    actions: Chord,
    settings: Chord,
    appearance: Chord,
    search_folders: Chord,
    search_status: Chord,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            actions: Chord { key: 'K', shift: false },
            settings: Chord { key: 'S', shift: true },
            appearance: Chord { key: ',', shift: true },
            search_folders: Chord { key: ',', shift: false },
            search_status: Chord { key: 'I', shift: false },
        }
    }
}

impl Preferences {
    fn binding(&mut self, action: Action) -> &mut Chord {
        match action {
            Action::Actions => &mut self.actions,
            Action::Settings => &mut self.settings,
            Action::Appearance => &mut self.appearance,
            Action::SearchFolders => &mut self.search_folders,
            Action::SearchStatus => &mut self.search_status,
        }
    }

    pub fn get(mut self, action: Action) -> Chord {
        *self.binding(action)
    }

    pub fn action(self, chord: Chord) -> Option<Action> {
        Action::ALL.into_iter().find(|&action| self.get(action) == chord)
    }

    pub fn set(&mut self, action: Action, chord: Chord) -> Result<(), String> {
        if let Some(other) = self.action(chord).filter(|&other| other != action) {
            return Err(format!("{chord} is already assigned to {}.", other.label()));
        }
        *self.binding(action) = chord;
        Ok(())
    }

    pub(crate) fn validate(self) -> Result<(), String> {
        for action in Action::ALL {
            let chord = self.get(action);
            if self.action(chord) != Some(action) {
                return Err(format!("{chord} is assigned more than once."));
            }
        }
        Ok(())
    }

    pub(crate) fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_accept_supported_chords_and_report_conflicts() {
        let mut prefs = Preferences::default();
        let chord = "Ctrl+Shift+u".parse().unwrap();
        prefs.set(Action::Actions, chord).unwrap();
        assert_eq!(chord.to_string(), "Ctrl+Shift+U");
        assert_eq!(prefs.action(chord), Some(Action::Actions));
        assert_eq!(prefs.action("Ctrl+K".parse().unwrap()), None);
        assert!(prefs.set(Action::Settings, chord).unwrap_err().contains("Actions"));
        assert_eq!(prefs.action(chord), Some(Action::Actions));
        for unsupported in ["A", "Ctrl+1", "Alt+K", "Ctrl+S", "Ctrl+Shift+C", "Ctrl+Shift+Enter"] {
            assert!(unsupported.parse::<Chord>().is_err(), "{unsupported}");
        }
    }

    #[test]
    fn keyboard_settings_save_only_their_section_and_keep_the_latest_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let unrelated = "# folders stay put\nroots = []\n\n[appearance]\nmode = 'light'\n";
        std::fs::write(&path, unrelated).unwrap();
        let original = crate::folders::Document::load(path.clone()).unwrap();
        let mut prefs = Preferences::default();
        prefs.set(Action::Actions, "Ctrl+Shift+U".parse().unwrap()).unwrap();
        let saved = original.save_shortcuts(prefs).unwrap();
        assert_eq!(crate::Config::load(&path).unwrap().shortcuts, prefs);
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("# folders stay put") && contents.contains("mode = 'light'"), "{contents}");
        assert!(original.save_shortcuts(prefs).unwrap_err().contains("changed on disk"));
        assert_eq!(saved.save_shortcuts(Preferences::default()).unwrap().config.shortcuts, Preferences::default());
        let duplicate = "[shortcuts]\nactions = 'Ctrl+I'\n";
        assert!(crate::Config::parse(duplicate).unwrap_err().to_string().contains("more than once"));
    }
}
