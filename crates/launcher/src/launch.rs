// SPDX-License-Identifier: GPL-3.0-or-later
//! Starting applications without a shell.
//!
//! As in the daemon's opener, a launched program gets its own transient
//! systemd user scope when a user manager is reachable, so it is not part of
//! the launcher's process tree accounting. Arguments are passed as argv
//! elements, never through a shell.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::apps::App;

/// Full argv for `app`, including the terminal wrapper and scope.
pub fn command_line(app: &App, scoped: bool, terminal: Option<&[String]>) -> io::Result<Vec<String>> {
    let mut argv = app.argv().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    if app.terminal {
        let Some(term) = terminal else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no terminal emulator found"));
        };
        let mut wrapped = term.to_vec();
        wrapped.append(&mut argv);
        argv = wrapped;
    }
    if scoped {
        let unit = format!("--unit=app-ferroseek-{}-{}", unit_safe(&app.id), std::process::id());
        let mut s: Vec<String> =
            ["systemd-run", "--user", "--scope", "--collect", "--quiet", "--slice=app.slice"].map(String::from).into();
        s.push(unit);
        s.push("--".into());
        s.append(&mut argv);
        argv = s;
    }
    Ok(argv)
}

/// systemd unit names allow `[A-Za-z0-9:_.\\-]`; escape the rest.
fn unit_safe(id: &str) -> String {
    id.trim_end_matches(".desktop")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '.') { c } else { '_' })
        .collect()
}

pub fn launch(app: &App) -> io::Result<()> {
    let argv = command_line(app, systemd_run_available(), terminal_command().as_deref())?;
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // Its own process group: a signal to the launcher's group (e.g. ^C in
    // the terminal it was started from) must not reach the application.
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    if let Some(dir) = app.working_dir.as_deref().filter(|d| d.is_dir()) {
        cmd.current_dir(dir);
    }
    // The launcher exits right after; the child is reparented, not reaped
    // here.
    cmd.spawn().map(drop)
}

fn in_path(prog: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(prog).is_file()))
}

fn systemd_run_available() -> bool {
    std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|d| Path::new(&d).join("systemd/private").exists())
        && in_path("systemd-run")
}

/// Terminal for `Terminal=true` entries: the freedesktop
/// `xdg-terminal-exec` if installed, else a known emulator with its
/// execute flag.
fn terminal_command() -> Option<Vec<String>> {
    if in_path("xdg-terminal-exec") {
        return Some(vec!["xdg-terminal-exec".into()]);
    }
    for (prog, flag) in [
        ("konsole", "-e"),
        ("gnome-terminal", "--"),
        ("kitty", "--"),
        ("alacritty", "-e"),
        ("foot", "--"),
        ("xterm", "-e"),
    ] {
        if in_path(prog) {
            return Some(vec![prog.into(), flag.into()]);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps;

    fn app(extra: &str) -> App {
        let text = format!("[Desktop Entry]\nType=Application\nName=T\nExec=prog --x %U\n{extra}");
        apps::parse("org.ex.T-1.desktop", Path::new("/a/org.ex.T-1.desktop"), &text, &[], &[]).unwrap()
    }

    #[test]
    fn plain_and_scoped_command_lines() {
        assert_eq!(command_line(&app(""), false, None).unwrap(), ["prog", "--x"]);
        let s = command_line(&app(""), true, None).unwrap();
        assert_eq!(&s[..6], ["systemd-run", "--user", "--scope", "--collect", "--quiet", "--slice=app.slice"]);
        assert!(s[6].starts_with("--unit=app-ferroseek-org.ex.T_1-"));
        assert_eq!(&s[7..], ["--", "prog", "--x"]);
    }

    #[test]
    fn terminal_entries_are_wrapped() {
        let t = app("Terminal=true");
        let term = vec!["konsole".to_string(), "-e".to_string()];
        assert_eq!(command_line(&t, false, Some(&term)).unwrap(), ["konsole", "-e", "prog", "--x"]);
        assert_eq!(command_line(&t, false, None).unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}
