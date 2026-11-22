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
mod commands;
mod desktop_settings;
mod details;
mod folders;
mod global_shortcuts;
mod icons;
mod input;
mod launch;
mod layout;
mod links;
mod material;
mod model;
mod open_with;
mod output;
mod preview;
mod saved;
mod script_commands;
mod shortcuts;
mod snippets;
mod timing;
mod ui;

use std::process::ExitCode;

use commands::DirectCommand;
use ferroseek_launcher::{clipboard_history, favorites, matcher, uri, usage, window_switcher};

const USAGE: &str = "\
Usage: ferroseek-launcher [--private] [--opaque] [--query TEXT | --command ID]
       ferroseek-launcher --list-commands | --forget-history | --help | --version
       ferroseek-launcher --global-shortcuts

Opens the launcher, or closes it when it is already open. Bind the command
to a global shortcut in the desktop settings.

  --query TEXT       start with TEXT already typed
  --command ID       open a built-in command or review a local command or workflow
  --list-commands    print built-in command IDs and titles
  --global-shortcuts listen for shortcuts assigned through the desktop portal
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
    destination: Destination,
    private: bool,
    opaque: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Destination {
    Search(String),
    Command(DirectCommand),
}

impl Destination {
    fn command(&self) -> Option<&DirectCommand> {
        match self {
            Self::Command(command) => Some(command),
            Self::Search(_) => None,
        }
    }
}

fn main() -> ExitCode {
    timing::init();
    let mut args = std::env::args().skip(1).peekable();
    if let Some(flag) = args.next_if(|arg| matches!(arg.as_str(), "--global-shortcuts" | "--serve-clipboard")) {
        if args.next().is_some() {
            return usage_error(&format!("{flag} is a standalone option"));
        }
        return if flag == "--global-shortcuts" { monitor_shortcuts() } else { serve_clipboard() };
    }
    match parse_options(args) {
        Ok(options) => launch_ui(options),
        Err(exit) => exit,
    }
}

fn parse_options(mut args: impl Iterator<Item = String>) -> Result<LaunchOptions, ExitCode> {
    let mut destination = None;
    let mut private = false;
    let mut opaque = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "-q" | "--query" | "--command" => destination = Some(parse_destination(&a, args.next(), destination)?),
            "--list-commands" => {
                for command in commands::ALL {
                    println!("{}\t{}", command.id(), command.title());
                }
                return Err(ExitCode::SUCCESS);
            }
            "--private" => private = true,
            "--global-shortcuts" => return Err(usage_error("--global-shortcuts is a standalone option")),
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
    Ok(LaunchOptions {
        destination: destination.unwrap_or_else(|| Destination::Search(String::new())),
        private,
        opaque,
    })
}

fn parse_destination(
    flag: &str,
    value: Option<String>,
    previous: Option<Destination>,
) -> Result<Destination, ExitCode> {
    let value = value.ok_or_else(|| usage_error(&format!("{flag} needs a value")))?;
    if previous.as_ref().is_some_and(|target| target.command().is_some() != (flag == "--command")) {
        return Err(usage_error("choose either --query or --command"));
    }
    if flag == "--command" {
        return DirectCommand::parse(&value)
            .map(Destination::Command)
            .ok_or_else(|| usage_error("invalid command ID. Use --list-commands or Copy launch command."));
    }
    Ok(Destination::Search(value))
}

fn launch_ui(LaunchOptions { destination, private, opaque }: LaunchOptions) -> ExitCode {
    // Applications are scanned while the bus connection is set up.
    let scan =
        std::thread::spawn(|| apps::scan(&apps::data_dirs(), &apps::current_locales(), &apps::current_desktops()));
    let activation = activation::Activation::default();
    let settings_path = ferroseek_config::Paths::from_env().config_file;
    let (backend, events) =
        match backend::start(activation.clone(), destination.command().cloned(), private, settings_path.clone()) {
            Ok(backend::Startup::Primary(b, ev)) => (b, ev),
            Ok(backend::Startup::Forwarded) => return ExitCode::SUCCESS,
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
    let paste = if private { None } else { window_switcher::PasteCapture::start() };
    let apps = scan.join().unwrap_or_default();
    timing::event("applications_ready");
    let history = if private { usage::Usage::disabled() } else { usage::Usage::load(usage::Usage::default_path()) };
    let favorites = if private {
        favorites::Favorites::disabled()
    } else {
        favorites::Favorites::load(favorites::Favorites::default_path())
    };
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
            settings_path,
            private,
            paste_target: paste.and_then(window_switcher::PasteCapture::finish),
        },
        destination,
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

fn serve_clipboard() -> ExitCode {
    match clipboard_history::publish::serve() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ferroseek-launcher: {error}");
            ExitCode::from(1)
        }
    }
}

fn monitor_shortcuts() -> ExitCode {
    match global_shortcuts::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ferroseek-launcher: {error}");
            ExitCode::from(1)
        }
    }
}

fn usage_error(msg: &str) -> ExitCode {
    eprintln!("ferroseek-launcher: {msg}\n\n{USAGE}");
    ExitCode::from(2)
}
