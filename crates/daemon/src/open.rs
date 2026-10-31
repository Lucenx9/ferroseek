// SPDX-License-Identifier: GPL-3.0-or-later
//! Opening files the way the desktop would, without a shell.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::runner::file_url;

/// Opens `path` with its default application via `xdg-open`, which defers
/// to the desktop (`kde-open` on Plasma, which uses KIO's application
/// association). The path is passed as a single argv element: no shell, no
/// interpolation. `token` is the XDG activation token from KRunner so
/// the opened window may take focus on Wayland.
pub fn open_path(path: &Path, token: Option<&str>) -> io::Result<()> {
    let mut cmd = Command::new("xdg-open");
    cmd.arg(path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(t) = token {
        cmd.env("XDG_ACTIVATION_TOKEN", t);
    }
    let mut child = cmd.spawn()?;
    // Reap without blocking the caller.
    std::thread::Builder::new().name("reap-xdg-open".into()).spawn(move || {
        let _ = child.wait();
    })?;
    Ok(())
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
