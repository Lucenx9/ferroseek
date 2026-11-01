// SPDX-License-Identifier: GPL-3.0-or-later
//! `ferroseek-launcher`: a keyboard-first launcher for applications and
//! FerroSeek file search, drawn on a Wayland layer-shell overlay.
//!
//! One instance at a time: running the command again while it is open
//! closes it, so a single desktop shortcut toggles the launcher.

mod apps;
mod backend;
mod calc;
mod icons;
mod launch;
mod matcher;
mod model;
mod shortcuts;
mod ui;
mod uri;
mod usage;

use std::process::ExitCode;

const USAGE: &str = "\
Usage: ferroseek-launcher [--private] [--query TEXT]
       ferroseek-launcher --forget-history | --help | --version

Opens the launcher, or closes it when it is already open. Bind the command
to a global shortcut in the desktop settings.

  --query TEXT       start with TEXT already typed
  --private          do not record what is opened in this session
  --forget-history   delete the launch history and exit

The launch history (what you open, used to rank results and to fill the
empty query) is stored only in $XDG_STATE_HOME/ferroseek/launcher-usage.

Keys: Up/Down or Tab move, Enter opens, Ctrl+Enter shows the file in its
folder, Ctrl+K lists actions, Ctrl+Shift+C copies the path, Esc clears the
query and then closes.";

fn main() -> ExitCode {
    let mut initial_query = String::new();
    let mut private = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-q" | "--query" => match args.next() {
                Some(q) => initial_query = q,
                None => return usage_error("--query needs a value"),
            },
            "--private" => private = true,
            "--forget-history" => {
                let Some(p) = usage::Usage::default_path() else { return usage_error("no HOME or XDG_STATE_HOME") };
                return match usage::Usage::forget(&p) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("ferroseek-launcher: cannot delete {}: {e}", p.display());
                        ExitCode::from(1)
                    }
                };
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("ferroseek-launcher {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            other => return usage_error(&format!("unknown argument {other:?}")),
        }
    }

    // Applications are scanned while the bus connection is set up.
    let scan =
        std::thread::spawn(|| apps::scan(&apps::data_dirs(), &apps::current_locales(), &apps::current_desktops()));
    let (backend, events) = match backend::start() {
        Ok(backend::Startup::Primary(b, ev)) => (b, ev),
        Ok(backend::Startup::Toggled) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ferroseek-launcher: {e}");
            return ExitCode::from(1);
        }
    };
    let apps = scan.join().unwrap_or_default();
    let history = if private { usage::Usage::disabled() } else { usage::Usage::load(usage::Usage::default_path()) };
    match ui::run(ui::Boot { backend, apps, usage: history, initial_query }, events) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ferroseek-launcher: {e}");
            ExitCode::from(1)
        }
    }
}

fn usage_error(msg: &str) -> ExitCode {
    eprintln!("ferroseek-launcher: {msg}\n\n{USAGE}");
    ExitCode::from(2)
}
