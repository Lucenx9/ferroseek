// SPDX-License-Identifier: GPL-3.0-or-later
//! `ferroseek-launcher`: a keyboard-first launcher for applications and
//! FerroSeek file search, drawn on a Wayland layer-shell overlay.
//!
//! One instance at a time: running the command again while it is open
//! closes it, so a single desktop shortcut toggles the launcher.

mod activation;
mod appearance;
mod apps;
mod backend;
mod calc;
mod clipboard_history;
mod commands;
mod desktop_settings;
mod details;
mod favorites;
mod folders;
mod icons;
mod input;
mod launch;
mod layout;
mod links;
mod matcher;
mod material;
mod model;
mod open_with;
mod output;
mod preview;
mod saved;
mod shortcuts;
mod snippets;
mod timing;
mod ui;
mod uri;
mod usage;
mod window_switcher;

use std::process::ExitCode;

const USAGE: &str = "\
Usage: ferroseek-launcher [--private] [--opaque] [--query TEXT]
       ferroseek-launcher --forget-history | --help | --version

Opens the launcher, or closes it when it is already open. Bind the command
to a global shortcut in the desktop settings.

  --query TEXT       start with TEXT already typed
  --opaque           use a solid surface without background blur
  --private          skip launch history and pause clipboard recording
  --forget-history   delete the launch history and exit

The launch history (what you open, used to rank results and to fill the
empty query) is stored only in $XDG_STATE_HOME/ferroseek/launcher-usage.
Favorites are stored separately beside it in launcher-favorites. Private
sessions read and write neither file.

Keys: Up/Down or Tab move, Enter opens, Ctrl+Enter shows the file in its
folder, Ctrl+K lists actions, Ctrl+Shift+C copies the path, Ctrl+Tab switches
search scopes, and Esc clears the query and then closes.";

struct LaunchOptions {
    initial_query: String,
    private: bool,
    opaque: bool,
}

fn main() -> ExitCode {
    timing::init();
    match parse_options() {
        Ok(options) => launch_ui(options),
        Err(exit) => exit,
    }
}

fn parse_options() -> Result<LaunchOptions, ExitCode> {
    let mut initial_query = String::new();
    let mut private = false;
    let mut opaque = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-q" | "--query" => match args.next() {
                Some(q) => initial_query = q,
                None => return Err(usage_error("--query needs a value")),
            },
            "--private" => private = true,
            "--watch-clipboard" => return Err(clipboard_watcher()),
            "--opaque" => opaque = true,
            "--forget-history" => {
                let Some(p) = usage::Usage::default_path() else {
                    return Err(usage_error("no HOME or XDG_STATE_HOME"));
                };
                return Err(match usage::Usage::forget(&p) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("ferroseek-launcher: cannot delete {}: {e}", p.display());
                        ExitCode::from(1)
                    }
                });
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Err(ExitCode::SUCCESS);
            }
            "-V" | "--version" => {
                println!("ferroseek-launcher {}", env!("CARGO_PKG_VERSION"));
                return Err(ExitCode::SUCCESS);
            }
            other => return Err(usage_error(&format!("unknown argument {other:?}"))),
        }
    }
    Ok(LaunchOptions { initial_query, private, opaque })
}

fn launch_ui(LaunchOptions { initial_query, private, opaque }: LaunchOptions) -> ExitCode {
    // Applications are scanned while the bus connection is set up.
    let scan =
        std::thread::spawn(|| apps::scan(&apps::data_dirs(), &apps::current_locales(), &apps::current_desktops()));
    let activation = activation::Activation::default();
    let (backend, events) = match backend::start(activation.clone()) {
        Ok(backend::Startup::Primary(b, ev)) => (b, ev),
        Ok(backend::Startup::Toggled) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ferroseek-launcher: {e}");
            return ExitCode::from(1);
        }
    };
    if let Err(error) = clipboard_history::launcher_started(private) {
        eprintln!("ferroseek-launcher: cannot pause clipboard recording: {error}");
        return ExitCode::from(1);
    }
    timing::event("backend_ready");
    let apps = scan.join().unwrap_or_default();
    timing::event("applications_ready");
    let history = if private { usage::Usage::disabled() } else { usage::Usage::load(usage::Usage::default_path()) };
    let favorites = if private {
        favorites::Favorites::disabled()
    } else {
        favorites::Favorites::load(favorites::Favorites::default_path())
    };
    let settings_path = ferroseek_config::Paths::from_env().config_file;
    let config = ferroseek_config::Config::load(&settings_path).unwrap_or_default();
    timing::event("ui_starting");
    match ui::run(
        ui::Boot {
            config,
            activation,
            opaque,
            backend,
            apps,
            usage: history,
            favorites,
            initial_query,
            settings_path,
            private,
        },
        events,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ferroseek-launcher: {e}");
            ExitCode::from(1)
        }
    }
}

fn clipboard_watcher() -> ExitCode {
    let Some(profile) = clipboard_history::Profile::from_env() else {
        return usage_error("no HOME or XDG_STATE_HOME");
    };
    match profile.watch() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ferroseek-launcher: clipboard recorder stopped: {error}");
            ExitCode::from(1)
        }
    }
}

fn usage_error(msg: &str) -> ExitCode {
    eprintln!("ferroseek-launcher: {msg}\n\n{USAGE}");
    ExitCode::from(2)
}
