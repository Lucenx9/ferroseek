// SPDX-License-Identifier: GPL-3.0-or-later
//! Icon theme lookup, cached per icon name.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use iced::widget::{image, svg};

#[derive(Debug, Clone)]
pub enum Icon {
    Svg(svg::Handle),
    Raster(image::Handle),
}

pub struct Icons {
    theme: String,
    size: u16,
    cache: HashMap<String, Option<Icon>>,
    /// shared-mime-info `generic-icons` keyed by MIME icon name, read when
    /// the first file row needs it.
    generic: Option<HashMap<String, String>>,
}

impl Icons {
    pub fn new(size: u16) -> Icons {
        Icons { theme: current_theme(), size, cache: HashMap::new(), generic: None }
    }

    #[cfg(test)]
    pub fn disabled() -> Self {
        Self { theme: String::new(), size: 0, cache: HashMap::new(), generic: Some(HashMap::new()) }
    }

    /// Icon for a theme name or an absolute file path, trying `fallbacks`
    /// in order when the name is missing from the theme.
    pub fn get(&mut self, name: &str, fallbacks: &[&str]) -> Option<Icon> {
        #[cfg(test)]
        if self.size == 0 {
            return None;
        }
        for n in std::iter::once(name).chain(fallbacks.iter().copied()) {
            if n.is_empty() {
                continue;
            }
            if let Some(i) = self.lookup(n) {
                return Some(i);
            }
        }
        None
    }

    fn lookup(&mut self, name: &str) -> Option<Icon> {
        if let Some(hit) = self.cache.get(name) {
            return hit.clone();
        }
        let path = if Path::new(name).is_absolute() {
            Some(PathBuf::from(name)).filter(|p| p.is_file())
        } else {
            freedesktop_icons::lookup(name)
                .with_size(self.size)
                .with_scale(1)
                .with_theme(&self.theme)
                .with_cache()
                .find()
        };
        let icon = path.and_then(|p| load(&p));
        self.cache.insert(name.to_string(), icon.clone());
        icon
    }

    /// The shared-mime-info generic icon for a MIME icon name missing from
    /// the theme: the database entry (`application-pdf` →
    /// `x-office-document`), else the media type's (`image-png` →
    /// `image-x-generic`). Themes such as Adwaita ship only these.
    pub fn generic_mime_icon(&mut self, name: &str) -> Option<String> {
        let table = self.generic.get_or_insert_with(|| load_generic_icons(&crate::apps::data_dirs()));
        if let Some(icon) = table.get(name) {
            return Some(icon.clone());
        }
        let (media, _) = name.split_once('-')?;
        Some(format!("{media}-x-generic"))
    }
}

/// `<data dir>/mime/generic-icons` lines `type/subtype:icon`, keyed like the
/// daemon's icon names (`/` → `-`). Earlier data dirs take precedence.
fn load_generic_icons(dirs: &[PathBuf]) -> HashMap<String, String> {
    let mut table = HashMap::new();
    for dir in dirs {
        let Ok(text) = std::fs::read_to_string(dir.join("mime/generic-icons")) else { continue };
        for (mime, icon) in text.lines().filter_map(|line| line.split_once(':')) {
            if !icon.is_empty() {
                table.entry(mime.replace('/', "-")).or_insert_with(|| icon.to_string());
            }
        }
    }
    table
}

fn load(p: &Path) -> Option<Icon> {
    match p.extension().and_then(|e| e.to_str()) {
        Some("svg" | "svgz") => Some(Icon::Svg(svg::Handle::from_path(p))),
        Some("png" | "xpm") => Some(Icon::Raster(image::Handle::from_path(p))),
        _ => None,
    }
}

/// The desktop's icon theme. Plasma stores it in `kdeglobals` (read only;
/// the launcher never writes desktop configuration); otherwise the GTK
/// setting, then Breeze/hicolor via the lookup's own fallback.
fn current_theme() -> String {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    if let Some(dir) = config
        && let Ok(text) = std::fs::read_to_string(dir.join("kdeglobals"))
        && let Some(t) = ini_value(&text, "Icons", "Theme")
    {
        return t;
    }
    freedesktop_icons::default_theme_gtk().unwrap_or_else(|| "breeze".to_string())
}

fn ini_value(text: &str, group: &str, key: &str) -> Option<String> {
    let header = format!("[{group}]");
    let mut in_group = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_group = line == header;
        } else if in_group
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            let v = v.trim();
            return (!v.is_empty()).then(|| v.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_kdeglobals_icon_theme() {
        let t = "[General]\nTheme=nope\n[Icons]\nTheme = Papirus-Dark\n[KDE]\nTheme=x\n";
        assert_eq!(ini_value(t, "Icons", "Theme").as_deref(), Some("Papirus-Dark"));
        assert_eq!(ini_value("[Icons]\nTheme=\n", "Icons", "Theme"), None);
        assert_eq!(ini_value("[General]\nTheme=a\n", "Icons", "Theme"), None);
    }

    #[test]
    fn mime_icons_use_the_database_generic_icon_then_their_media_type() {
        let temp = tempfile::tempdir().unwrap();
        let (user, system) = (temp.path().join("user"), temp.path().join("system"));
        for (dir, text) in [
            (&user, "application/pdf:x-office-document\n"),
            (&system, "application/pdf:text-x-generic\napplication/zip:package-x-generic\nbroken\n"),
        ] {
            std::fs::create_dir_all(dir.join("mime")).unwrap();
            std::fs::write(dir.join("mime/generic-icons"), text).unwrap();
        }
        let mut icons = Icons { generic: Some(load_generic_icons(&[user, system])), ..Icons::disabled() };
        assert_eq!(icons.generic_mime_icon("application-pdf").as_deref(), Some("x-office-document"), "user dir wins");
        assert_eq!(icons.generic_mime_icon("application-zip").as_deref(), Some("package-x-generic"));
        assert_eq!(icons.generic_mime_icon("image-png").as_deref(), Some("image-x-generic"));
        assert_eq!(icons.generic_mime_icon("unknown"), None);
    }
}
