// SPDX-License-Identifier: GPL-3.0-or-later
//! `ferroseek`: command-line interface to the FerroSeek index and daemon.

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ferroseek_engine::{Config, EngineError, Paths, index_once, ipc};
use ferroseek_index::{Kind, Query, Reader, SearchOptions};

#[derive(Parser)]
#[command(name = "ferroseek", version, about = "Fast, lightweight file name search for Linux")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Search file and directory names. All words must match
    /// (case-insensitive substrings); words containing '/' match the path.
    #[command(alias = "s")]
    Search {
        #[arg(required = true)]
        query: Vec<String>,
        /// Maximum number of results.
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
        /// Only results below this directory.
        #[arg(long = "in", value_name = "DIR")]
        within: Option<PathBuf>,
        /// Only directories.
        #[arg(long, conflicts_with = "files")]
        dirs: bool,
        /// Only regular files.
        #[arg(long)]
        files: bool,
        /// Exclude dotfiles and anything below dot-directories.
        #[arg(long)]
        no_hidden: bool,
        /// Show kind, size, modification time and score.
        #[arg(short, long)]
        long: bool,
        /// Separate results with NUL (for xargs -0); paths are raw bytes.
        #[arg(short = '0', long)]
        null: bool,
    },
    /// Show daemon and index status.
    Status,
    /// Pause indexing (the daemon keeps answering queries).
    Pause,
    /// Resume indexing; changes made while paused are reconciled.
    Resume,
    /// Reconcile the index with the filesystem now. Without a running
    /// daemon, indexes once in the foreground.
    Index {
        /// Re-stat every file instead of skipping unchanged directories.
        #[arg(long)]
        full: bool,
    },
    /// Discard the index and build it again from scratch.
    Rebuild,
    /// Print the effective configuration and file locations.
    Config {
        /// Print the built-in defaults instead of the effective config.
        #[arg(long)]
        default: bool,
    },
}

fn die(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("ferroseek: {msg}");
    ExitCode::from(2)
}

/// Whether the daemon currently owns its bus name. Checked first so that
/// CLI calls never D-Bus-activate the daemon as a side effect.
async fn daemon_present(conn: &zbus::Connection) -> zbus::Result<bool> {
    conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "NameHasOwner",
        &(ipc::BUS_NAME,),
    )
    .await?
    .body()
    .deserialize::<bool>()
}

/// Calls a control method. `Ok(None)`: no daemon is running.
fn daemon_call(method: &str, full: Option<bool>) -> Result<Option<zbus::Message>, String> {
    zbus::block_on(async {
        let conn = zbus::Connection::session().await.map_err(|e| format!("session bus unavailable: {e}"))?;
        if !daemon_present(&conn).await.map_err(|e| e.to_string())? {
            return Ok(None);
        }
        let dest = Some(ipc::BUS_NAME);
        let r = match full {
            Some(f) => conn.call_method(dest, ipc::CONTROL_PATH, Some(ipc::CONTROL_IFACE), method, &(f,)).await,
            None => conn.call_method(dest, ipc::CONTROL_PATH, Some(ipc::CONTROL_IFACE), method, &()).await,
        };
        r.map(Some).map_err(|e| e.to_string())
    })
}

fn daemon_status() -> Result<Option<HashMap<String, String>>, String> {
    match daemon_call("Status", None)? {
        Some(m) => m.body().deserialize::<HashMap<String, String>>().map(Some).map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

fn kind_char(k: Kind) -> char {
    match k {
        Kind::File => 'f',
        Kind::Dir => 'd',
        Kind::Symlink => 'l',
        Kind::Other => '?',
    }
}

fn fmt_time(ns: i64) -> String {
    // UTC, dependency-free (civil-from-days, Howard Hinnant).
    let secs = ns.div_euclid(1_000_000_000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

fn human(bytes: u64) -> String {
    let units = ["B", "K", "M", "G", "T"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < units.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{bytes}B") } else { format!("{v:.1}{}", units[u]) }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let paths = Paths::from_env();
    match cli.cmd {
        Cmd::Search { query, limit, within, dirs, files, no_hidden, long, null } => {
            let reader = match Reader::open(&paths.index_file()) {
                Ok(r) => r,
                Err(e) => {
                    return die(format!(
                        "cannot open index {} ({e}); run `ferroseek index` or start ferroseekd",
                        paths.index_file().display()
                    ));
                }
            };
            let within = within.map(|w| std::fs::canonicalize(&w).unwrap_or(w));
            let opts = SearchOptions {
                limit,
                within,
                kind: if dirs { Some(Kind::Dir) } else if files { Some(Kind::File) } else { None },
                include_hidden: !no_hidden,
            };
            let hits = match reader.search(&Query::parse(&query.join(" ")), &opts) {
                Ok(h) => h,
                Err(e) => return die(e),
            };
            let mut out = std::io::stdout().lock();
            for h in &hits {
                let r = if long {
                    writeln!(
                        out,
                        "{} {:>7} {} {:6.1}  {}",
                        kind_char(h.kind),
                        if h.kind == Kind::Dir { "-".into() } else { human(h.size) },
                        fmt_time(h.mtime_ns),
                        h.score,
                        h.path.display()
                    )
                } else {
                    out.write_all(h.path.as_os_str().as_bytes()).and_then(|_| out.write_all(if null { b"\0" } else { b"\n" }))
                };
                if r.is_err() {
                    return ExitCode::SUCCESS; // broken pipe (e.g. `| head`)
                }
            }
            if hits.is_empty() { ExitCode::from(1) } else { ExitCode::SUCCESS }
        }
        Cmd::Status => {
            let daemon = daemon_status();
            match daemon {
                Ok(Some(m)) => {
                    let mut keys: Vec<_> = m.keys().cloned().collect();
                    keys.sort();
                    println!("daemon: running (pid {})", m.get("pid").map(String::as_str).unwrap_or("?"));
                    for k in keys.iter().filter(|k| *k != "pid") {
                        let v = &m[k];
                        if k == "disk_bytes" {
                            println!("  {k}: {v} ({})", human(v.parse().unwrap_or(0)));
                        } else {
                            println!("  {k}: {v}");
                        }
                    }
                }
                Ok(None) | Err(_) => {
                    if let Err(e) = &daemon {
                        println!("daemon: unknown ({e})");
                    } else {
                        println!("daemon: not running");
                    }
                    match Reader::open(&paths.index_file()).and_then(|r| r.stats()) {
                        Ok(s) => {
                            println!("  index_path: {}", paths.index_file().display());
                            println!("  files: {}\n  dirs: {}\n  disk_bytes: {} ({})", s.files, s.dirs, s.disk_bytes, human(s.disk_bytes));
                            for r in s.roots {
                                println!("  root: {}", r.display());
                            }
                        }
                        Err(e) => println!("  no index at {} ({e})", paths.index_file().display()),
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Cmd::Pause | Cmd::Resume => {
            let m = if matches!(cli.cmd, Cmd::Pause) { "Pause" } else { "Resume" };
            match daemon_call(m, None) {
                Ok(Some(_)) => {
                    println!("{}", if m == "Pause" { "indexing paused" } else { "indexing resumed" });
                    ExitCode::SUCCESS
                }
                Ok(None) => die("the daemon is not running"),
                Err(e) => die(e),
            }
        }
        Cmd::Index { .. } | Cmd::Rebuild => {
            let (full, rebuild) = match cli.cmd {
                Cmd::Index { full } => (full, false),
                _ => (true, true),
            };
            let r = if rebuild { daemon_call("Rebuild", None) } else { daemon_call("Reindex", Some(full)) };
            match r {
                Ok(Some(_)) => {
                    println!("{} requested from the daemon; see `ferroseek status`", if rebuild { "rebuild" } else { "reindex" });
                    return ExitCode::SUCCESS;
                }
                Ok(None) | Err(_) => {}
            }
            let cfg = match Config::load(&paths.config_file) {
                Ok(c) => c,
                Err(e) => return die(e),
            };
            eprintln!("daemon not running; indexing in the foreground…");
            let t = std::time::Instant::now();
            match index_once(&cfg, &paths, full, rebuild) {
                Ok(s) => {
                    println!(
                        "indexed {} files, {} directories in {:.2}s ({})",
                        s.files,
                        s.dirs,
                        t.elapsed().as_secs_f64(),
                        human(s.disk_bytes)
                    );
                    ExitCode::SUCCESS
                }
                Err(EngineError::Busy) => die("another FerroSeek process holds the index"),
                Err(e) => die(e),
            }
        }
        Cmd::Config { default } => {
            let cfg = if default {
                Config::default()
            } else {
                match Config::load(&paths.config_file) {
                    Ok(c) => c,
                    Err(e) => return die(e),
                }
            };
            println!("# config file: {}{}", paths.config_file.display(), if paths.config_file.exists() { "" } else { " (absent: defaults)" });
            println!("# index:       {}", paths.index_file().display());
            println!("# roots:       {}", cfg.root_paths().iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "));
            print!("{}", cfg.to_toml());
            ExitCode::SUCCESS
        }
    }
}
