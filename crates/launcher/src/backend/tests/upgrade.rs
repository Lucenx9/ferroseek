// SPDX-License-Identifier: GPL-3.0-or-later
//! Run against two real daemon builds as described in docs/INSTALLATION.md.

use super::{Activation, Backend, TestChild, Timeouts, mpsc, private_bus, serve};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn start_daemon(binary: &Path, home: &Path, address: &str) -> TestChild {
    TestChild(
        Command::new(binary)
            .env_clear()
            .env("HOME", home)
            .env("XDG_RUNTIME_DIR", home)
            .env("XDG_DATA_DIRS", home.join("empty"))
            .env("XDG_CONFIG_DIRS", home.join("empty"))
            .env("DBUS_SESSION_BUS_ADDRESS", address.trim())
            .env("FERROSEEK_CONFIG", home.join("config.toml"))
            .env("FERROSEEK_DATA_DIR", home.join("data"))
            .env("PATH", home.join("bin"))
            .spawn()
            .unwrap(),
    )
}

async fn wait_for_index(daemon: &mut TestChild, backend: &Backend) {
    let runtime = zruntime::SharedRuntime::current().unwrap();
    runtime
        .timeout(Duration::from_secs(15), async {
            loop {
                assert!(daemon.0.try_wait().unwrap().is_none(), "daemon stopped during startup");
                if backend.clone().status().await.is_ok_and(|s| s.state == "idle" && s.files == 1) {
                    break;
                }
                runtime.sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("daemon did not index the fixture");
}

async fn stop_daemon(mut daemon: TestChild) {
    assert!(Command::new("kill").arg("-TERM").arg(daemon.0.id().to_string()).status().unwrap().success());
    let runtime = zruntime::SharedRuntime::current().unwrap();
    runtime
        .timeout(Duration::from_secs(10), async {
            loop {
                if let Some(exit) = daemon.0.try_wait().unwrap() {
                    assert!(exit.success(), "daemon shutdown: {exit}");
                    break;
                }
                runtime.sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("daemon did not stop");
}

#[test]
#[ignore = "requires the two daemon builds documented in docs/INSTALLATION.md"]
fn launcher_recovers_after_daemon_upgrade() {
    let old = std::env::var_os("FERROSEEK_OLD_DAEMON").expect("set FERROSEEK_OLD_DAEMON");
    let current = std::env::var_os("FERROSEEK_CURRENT_DAEMON").expect("set FERROSEEK_CURRENT_DAEMON");
    let (temp, _bus, address) = private_bus();
    let home = temp.path();
    let root = home.join("corpus");
    let folder = root.join("Invoices");
    let file = folder.join("Quarterly report.txt");
    fs::create_dir_all(&folder).unwrap();
    fs::write(&file, "synthetic report").unwrap();
    fs::write(home.join("config.toml"), format!("roots = [{root:?}]\n")).unwrap();
    fs::create_dir(home.join("bin")).unwrap();
    let opener = home.join("bin/xdg-open");
    fs::write(
        &opener,
        r#"#!/bin/sh
printf '%s\n' "$@" > "$HOME/open-args"
: > "$HOME/opened"
"#,
    )
    .unwrap();
    fs::set_permissions(opener, fs::Permissions::from_mode(0o700)).unwrap();

    zbus::block_on(async {
        let conn = zbus::connection::Builder::address(address.trim()).build().await.unwrap();
        let (tx, rx) = mpsc::unbounded();
        let backend = Backend { tx };
        let verify = async move {
            let mut daemon = start_daemon(old.as_ref(), home, &address);
            wait_for_index(&mut daemon, &backend).await;
            let folders = backend.clone().search("Invoices".into()).await.unwrap().unwrap();
            let folder_id =
                folders.into_iter().find(|hit| hit.is_dir && hit.path.as_ref() == Some(&folder)).unwrap().id;
            assert_eq!(
                backend.clone().list_directory(folder_id.clone()).await.unwrap_err(),
                "Folder navigation requires an updated daemon."
            );

            let hits = backend.clone().search("Quarterly".into()).await.unwrap().unwrap();
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].path.as_ref(), Some(&file));
            backend.clone().run(hits[0].id.clone(), "").await.unwrap();
            let runtime = zruntime::SharedRuntime::current().unwrap();
            runtime
                .timeout(Duration::from_secs(5), async {
                    while !home.join("opened").exists() {
                        runtime.sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("old daemon opener did not finish recording its arguments");
            assert_eq!(fs::read_to_string(home.join("open-args")).unwrap(), format!("{}\n", file.display()));

            stop_daemon(daemon).await;
            let mut replacement = start_daemon(current.as_ref(), home, &address);
            wait_for_index(&mut replacement, &backend).await;
            let (entries, truncated) = backend.list_directory(folder_id).await.unwrap();
            assert!(!truncated);
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].id, hits[0].id);
            stop_daemon(replacement).await;
        };
        iced::futures::future::join(serve(conn, rx, Timeouts::default(), Activation::default()), verify).await;
    });
}
