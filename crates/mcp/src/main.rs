// SPDX-License-Identifier: GPL-3.0-or-later
//! `ferroseek-mcp`: optional, read-only MCP server (stdio) over the
//! FerroSeek index. It starts no indexer, opens the database read-only, and
//! exits when the client closes stdin. Nothing runs when no agent uses it.

mod beneath;
mod policy;
mod server;

use std::path::PathBuf;
use std::process::ExitCode;

use ferroseek_engine::Paths;
use rmcp::ServiceExt;

const HELP: &str = "\
ferroseek-mcp — read-only MCP server (stdio) for the FerroSeek file index

USAGE: ferroseek-mcp [--access-mode MODE] [--allow DIR]... [--config FILE] [--index FILE]

Only indexed paths can be returned. New unconfigured servers default to all_indexed;
dotfiles and secret-looking names (keys, .env, .ssh, ...) stay hidden.

  --access-mode MODE  all_indexed (default), home, or restricted
  --allow DIR    authorize DIR (repeatable; adds to `allow` in the config)
  --config FILE  policy file (default: $XDG_CONFIG_HOME/ferroseek/mcp.toml)
  --index FILE   index database (default: $XDG_DATA_HOME/ferroseek/index.sqlite)

The protocol runs on stdin/stdout; diagnostics go to stderr. No network.";

fn default_config() -> PathBuf {
    let base = match std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        Some(p) if p.is_absolute() => p,
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"),
    };
    base.join("ferroseek/mcp.toml")
}

fn main() -> ExitCode {
    // Few threads ever allocate; without a cap every short-lived blocking
    // thread gets its own glibc arena whose freed pages linger as RSS.
    #[cfg(target_env = "gnu")]
    // SAFETY: mallopt before any other thread exists.
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 2);
    }
    let mut allow = Vec::new();
    let mut mode = None;
    let mut config: Option<PathBuf> = None;
    let mut index: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        let r = match a.as_str() {
            "--access-mode" => {
                value("--access-mode").and_then(|v| policy::AccessMode::parse(&v)).map(|v| mode = Some(v))
            }
            "--allow" => value("--allow").map(|v| allow.push(v)),
            "--config" => value("--config").map(|v| config = Some(v.into())),
            "--index" => value("--index").map(|v| index = Some(v.into())),
            "--help" | "-h" => {
                println!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "--version" | "-V" => {
                println!("ferroseek-mcp {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            other => Err(format!("unknown argument {other:?}")),
        };
        if let Err(e) = r {
            eprintln!("ferroseek-mcp: {e}\n\n{HELP}");
            return ExitCode::from(2);
        }
    }

    let config_path = config.clone().unwrap_or_else(default_config);
    let mut file_cfg = match std::fs::read_to_string(&config_path) {
        Ok(s) => match toml::from_str::<policy::FileConfig>(&s) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("ferroseek-mcp: invalid {}: {e}", config_path.display());
                return ExitCode::from(2);
            }
        },
        // A missing default file is fine; an explicitly named one is not.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && config.is_none() => policy::FileConfig::default(),
        Err(e) => {
            eprintln!("ferroseek-mcp: cannot read {}: {e}", config_path.display());
            return ExitCode::from(2);
        }
    };
    if let Some(mode) = mode {
        file_cfg.access_mode = mode;
    } else if !allow.is_empty() {
        // A legacy --allow registration must never gain global access after
        // upgrading from the old restricted-by-default version.
        file_cfg.access_mode = policy::AccessMode::Restricted;
    }
    let policy = match policy::Policy::new(&file_cfg, &allow) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ferroseek-mcp: {e}");
            return ExitCode::from(2);
        }
    };
    if policy.allowed_dirs().is_empty() {
        eprintln!("ferroseek-mcp: warning: no directories authorized; tools will refuse every path (see --help)");
    }
    let db = index.unwrap_or_else(|| Paths::from_env().index_file());
    let service = server::FerroSeekMcp::new(policy, server::IndexAccess::new(db));

    // One async thread for the protocol; SQLite work goes to a small,
    // bounded blocking pool whose threads exit after 10 s idle.
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(4)
        .thread_keep_alive(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("ferroseek-mcp: cannot start runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let r = rt.block_on(async move {
        service.spawn_idle_release();
        let running = service.serve(rmcp::transport::stdio()).await.map_err(|e| e.to_string())?;
        running.waiting().await.map_err(|e| e.to_string())?;
        Ok::<_, String>(())
    });
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ferroseek-mcp: {e}");
            ExitCode::FAILURE
        }
    }
}
