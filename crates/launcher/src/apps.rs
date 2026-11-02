// SPDX-License-Identifier: GPL-3.0-or-later
//! Cached matching labels for installed desktop applications.
use crate::matcher;
pub use ferroseek_desktop::apps::{current_desktops, current_locales, data_dirs};
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    entry: ferroseek_desktop::apps::App,
    search: Vec<(String, u32)>,
}
impl std::ops::Deref for App {
    type Target = ferroseek_desktop::apps::App;
    fn deref(&self) -> &Self::Target {
        &self.entry
    }
}
#[cfg(test)]
impl std::ops::DerefMut for App {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entry
    }
}
impl App {
    /// Matching score for query tokens, or `None`.
    pub fn score(&self, tokens: &[String]) -> Option<u32> {
        // The whole name typed out is an exact match even when it has
        // several words (token scores alone would average below exact).
        if !tokens.is_empty() && tokens.iter().map(String::as_str).eq(self.search[0].0.split_whitespace()) {
            return Some(matcher::EXACT);
        }
        let labels: Vec<(&str, u32)> = self.search.iter().map(|(l, w)| (l.as_str(), *w)).collect();
        matcher::score(tokens, &labels)
    }
}
fn cached(entry: ferroseek_desktop::apps::App) -> App {
    let mut search = vec![(entry.name.to_lowercase(), 100)];
    if let Some(g) = &entry.generic_name {
        search.push((g.to_lowercase(), 70));
    }
    for k in &entry.keywords {
        search.push((k.to_lowercase(), 60));
    }
    // The ID's last component ("org.kde.dolphin" → "dolphin") finds apps
    // by their executable-like name.
    let stem = entry.id.trim_end_matches(".desktop");
    let short = stem.rsplit('.').next().unwrap_or(stem).to_lowercase();
    if short != entry.name.to_lowercase() {
        search.push((short, 60));
    }
    if let Some(c) = &entry.comment {
        search.push((c.to_lowercase(), 40));
    }
    App { entry, search }
}
pub fn scan(dirs: &[PathBuf], locales: &[String], desktops: &[String]) -> Vec<App> {
    ferroseek_desktop::apps::scan(dirs, locales, desktops).into_iter().map(cached).collect()
}
#[cfg(test)]
pub fn parse(id: &str, file: &Path, text: &str, locales: &[String], desktops: &[String]) -> Option<App> {
    ferroseek_desktop::apps::parse(id, file, text, locales, desktops).map(cached)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_application_labels_remain_searchable() {
        let app = parse(
            "org.example.App.desktop",
            Path::new("/synthetic/app.desktop"),
            "[Desktop Entry]\nType=Application\nName=Viewer\nExec=viewer\nKeywords=explorer;\n",
            &[],
            &[],
        )
        .unwrap();
        assert!(app.score(&matcher::tokens("explorer")).is_some());
        assert!(app.score(&matcher::tokens("app")).is_some());
    }
}
