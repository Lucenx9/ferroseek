// SPDX-License-Identifier: GPL-3.0-or-later
//! Opening files the way the desktop would, without a shell.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::search::file_url;

/// Opens `path` with its default application via `xdg-open`, which defers
/// to the desktop (`kde-open` on Plasma, which uses KIO's application
/// association). The path is passed as a single argv element: no shell, no
/// interpolation. `token` is the XDG activation token from the launcher so
/// the opened window may take focus on Wayland.
///
/// When systemd is available the opener runs in its own transient user
/// scope (`systemd-run --user --scope`), so the launched application is
/// not a member of `ferroseek.service`: daemon restarts or its resource
/// limits must never affect applications the user opened.
pub fn open_path(path: &Path, token: Option<&str>) -> io::Result<()> {
    let scoped = systemd_run_available();
    let mut cmd = if scoped {
        let mut c = Command::new("systemd-run");
        c.args(["--user", "--scope", "--collect", "--quiet", "--slice=app.slice", "--", "xdg-open"]);
        c
    } else {
        Command::new("xdg-open")
    };
    cmd.arg(path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd.env_remove("XDG_ACTIVATION_TOKEN").env_remove("DESKTOP_STARTUP_ID");
    if let Some(t) = token.filter(|token| !token.is_empty()) {
        cmd.env("XDG_ACTIVATION_TOKEN", t);
    }
    let mut child = cmd.spawn()?;
    // Reap without blocking the caller.
    std::thread::Builder::new().name("reap-opener".into()).spawn(move || {
        let _ = child.wait();
    })?;
    Ok(())
}

fn systemd_run_available() -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::sync::OnceLock;
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        // A user manager must be reachable, not just the binary present.
        std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|d| Path::new(&d).join("systemd/private").exists())
            && std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).any(|d| {
                let program = d.join("systemd-run");
                program.is_file()
                    && CString::new(program.as_os_str().as_bytes()).is_ok_and(|program| {
                        // SAFETY: program is a NUL-terminated path valid for this call.
                        unsafe { libc::faccessat(libc::AT_FDCWD, program.as_ptr(), libc::X_OK, libc::AT_EACCESS) == 0 }
                    })
            })
    })
}

/// Shows `path` selected in the file manager
/// (`org.freedesktop.FileManager1.ShowItems`, implemented by Dolphin and
/// others), falling back to opening the parent directory.
pub async fn show_in_folder(conn: &zbus::Connection, path: &Path, token: Option<&str>) -> io::Result<()> {
    let urls = vec![file_url(path)];
    let r = conn
        .call_method(
            Some("org.freedesktop.FileManager1"),
            "/org/freedesktop/FileManager1",
            Some("org.freedesktop.FileManager1"),
            "ShowItems",
            &(urls, token.unwrap_or("")),
        )
        .await;
    match r {
        Ok(_) => Ok(()),
        Err(e) => {
            log::debug!("FileManager1.ShowItems failed ({e}); opening parent directory");
            open_path(path.parent().unwrap_or(Path::new("/")), token)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::time::{Duration, Instant};

    #[test]
    fn opening_falls_back_when_systemd_run_is_not_executable() {
        const CASE: &str = "FERROSEEK_TEST_OPENER_SCOPE";
        if let Some(case) = std::env::var_os(CASE) {
            let output = std::env::var_os("FERROSEEK_TEST_OPEN_OUTPUT").unwrap();
            let output = Path::new(&output);
            open_path(Path::new("/synthetic/two words.txt"), None).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !output.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            let args = fs::read_to_string(output).unwrap();
            if case == "executable" {
                assert_eq!(
                    args,
                    "--user\n--scope\n--collect\n--quiet\n--slice=app.slice\n--\nxdg-open\n/synthetic/two words.txt\n"
                );
            } else {
                assert_eq!(args, "/synthetic/two words.txt\n");
            }
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        fs::create_dir_all(runtime.join("systemd")).unwrap();
        fs::write(runtime.join("systemd/private"), "").unwrap();
        let capture = "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$FERROSEEK_TEST_OPEN_OUTPUT.tmp\"\n/bin/mv \"$FERROSEEK_TEST_OPEN_OUTPUT.tmp\" \"$FERROSEEK_TEST_OPEN_OUTPUT\"\n";
        let opener = temp.path().join("xdg-open");
        fs::write(&opener, capture).unwrap();
        fs::set_permissions(opener, fs::Permissions::from_mode(0o700)).unwrap();
        let scope = temp.path().join("systemd-run");
        for case in ["missing", "not-executable", "not-executable-for-owner", "executable"] {
            // Root can execute a regular file when any execute bit is set.
            if case == "not-executable-for-owner" && fs::metadata(temp.path()).unwrap().uid() == 0 {
                continue;
            }
            if case != "missing" {
                fs::write(&scope, capture).unwrap();
                let mode = match case {
                    "executable" => 0o700,
                    "not-executable-for-owner" => 0o601,
                    _ => 0o600,
                };
                fs::set_permissions(&scope, fs::Permissions::from_mode(mode)).unwrap();
            }
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "open::tests::opening_falls_back_when_systemd_run_is_not_executable", "--nocapture"])
                .env(CASE, case)
                .env("FERROSEEK_TEST_OPEN_OUTPUT", temp.path().join(case))
                .env("XDG_RUNTIME_DIR", &runtime)
                .env("PATH", temp.path())
                .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent-ferroseek-test-bus")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "case={case}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
