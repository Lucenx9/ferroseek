# FerroSeek

**Lightweight, real-time filename and path search for Linux.** FerroSeek is a Rust-powered, desktop-independent search engine that maintains a local index and exposes it through a CLI, optional KDE Plasma KRunner and Noctalia launcher integrations, and an optional MCP server for AI agents.

> **Status: 0.1 MVP (development).** Filename/path search, live indexing, CLI, KRunner and MCP work. There is **no published binary release yet**. File-content search, GNOME/Rofi integrations and full Baloo API compatibility are not implemented.

## Features

- **Find files and directories by name or path**, with case-insensitive substring matching and ranked results.
- **Keep an index up to date** using inotify, incremental reconciliation and crash recovery.
- **Low background overhead** through bounded queues, throttled scans and cache release while idle.
- **One shared SQLite FTS5 index** used by the CLI, KRunner and read-only MCP server.
- **Desktop-independent core**: one-shot indexing and search work on headless Linux without KDE, root privileges or telemetry.

FerroSeek stores filenames, paths and basic filesystem metadata (type, size, modification time). **It does not search file contents**; `rg` remains useful for content/regex searches.

## Quick start

Clone the source repository and use the user-scoped installer:

```sh
git clone https://github.com/Lucenx9/ferroseek.git
cd ferroseek
./scripts/install.sh               # guided installation and recommendations
```

The source installer needs **Python 3.11+** and **Rust 1.89+** with Cargo and a C linker. It runs without sudo and does not download remote scripts. Prebuilt local archive packaging exists, but release binaries have not been published.

For scripted or selective installation:

```sh
./scripts/install.sh --core        # daemon + CLI, including headless Linux
./scripts/install.sh --krunner     # core + KDE Plasma integration
./scripts/install.sh --mcp         # core + optional MCP executable
./scripts/install.sh --full        # all implemented, compatible components
./scripts/install.sh --dry-run     # inspect planned changes without writing
```

Flags may be combined. Installation preserves existing indexes and configurations, does not enable indexing automatically, and does not restart an existing service. Review your indexing roots before starting; the engine defaults to your home with exclusions. To start indexing explicitly on systems with a user systemd manager and session bus:

```sh
./scripts/install.sh --core --enable
ferroseek status
```

For headless use, `ferroseek index` and `ferroseek search` work without a running daemon. See the [installation guide](docs/INSTALLATION.md) for XDG paths, upgrades, archive verification, rollback and uninstall.

## Ways to search

| Interface | Functionality | Status |
|---|---|---|
| **CLI** (`ferroseek`) | Search, index, status and maintenance commands | Core, available |
| **KRunner** | File results from Alt+Space in KDE Plasma | Optional, verified on Plasma 6 |
| **MCP** (`ferroseek-mcp`) | Read-only name/path search and metadata for AI agents | Optional, tested with Claude Code and Codex |
| **Noctalia v5** | Optional `/ferro` launcher plugin | Source-checkout adapter; [setup and validation limits](docs/NOCTALIA.md) |
| **GNOME, Rofi and other launchers** | Dedicated desktop adapters | Planned, **not available yet** |

### Command line

```sh
ferroseek index                       # reconcile an index without a daemon
ferroseek search report               # find names containing "report"
ferroseek search --in ~/Documents cv  # restrict search to a directory
ferroseek search src/ main.rs          # path fragment and filename
ferroseek search -n 50 --long report  # metadata and ranking
ferroseek search --json report        # ranked machine-readable results
ferroseek status                      # daemon and index statistics
ferroseek config --default            # show default configuration
```

The engine config is at `$XDG_CONFIG_HOME/ferroseek/config.toml` (normally `~/.config/ferroseek/config.toml`); index data is at `$XDG_DATA_HOME/ferroseek`. Roots, exclusions, scan limits and reconciliation intervals are configurable. See [installation](docs/INSTALLATION.md).

### KDE Plasma

Install `--krunner` to surface FerroSeek's results in **KRunner (Alt+Space)**. Results can open a file or show its containing directory. This adapter does not replace KRunner or modify KDE/Baloo configuration. If the existing KDE file-search runner also returns matches, you can disable that runner manually in **System Settings → Search → Plasma Search**.

### Noctalia v5

The optional `lucenx9/ferroseek` plugin adds `/ferro` file search to Noctalia's
launcher, including on Niri. It reuses the CLI and existing index, with no
second background service. Installation is separate from the core installer;
see the [Noctalia guide](docs/NOCTALIA.md) for setup, requirements and validation
limits.

### AI agents (optional MCP)

The separate `ferroseek-mcp` program uses local stdio, reads the existing index and exits when its client disconnects. It exposes three read-only tools: `search_files`, `get_file_info`, and `index_status`. No second indexer is started.

Installing MCP does **not** register an agent or change its configuration.
However, an unconfigured MCP executable now defaults to `all_indexed` when
started, so review existing manual registrations without an access mode during
an upgrade:

```sh
# Authorize one project for Claude Code
./scripts/install.sh --mcp --register-claude --allow "$HOME/Projects/example"

# Explicitly authorize discovery across all already indexed roots for Codex
./scripts/install.sh --mcp --register-codex --mcp-access all_indexed
```

Access modes are `all_indexed` (**new default**), `home`, and `restricted` (explicit allowlists). Guided global registration requires typed confirmation. Legacy `--allow` registrations and old TOML policy files without an explicit mode remain restricted; a previously unconfigured MCP server will gain indexed-path access after upgrading. Hidden and sensitive-looking names remain filtered according to MCP policy. Search permissions do not change the indexer's roots. MCP tools are *available* to agents, but [independent selection tests](docs/MCP_DISCOVERY.md) show that agents often prefer their built-in search tools.

Read the [MCP guide](docs/MCP.md) for configuration and security.

## Measured performance and limitations

On one Ryzen 7 5800X, NVMe and Btrfs machine, an index containing approximately **953,595 files** measured **10.1 MiB idle daemon RSS after restart**. A selective million-file path-query fixture improved from approximately **530 ms to 2.03 ms** in paired measurements. These numbers apply to the described workload; they are **not** universal memory, latency or throughput guarantees. See the [benchmark methodology and raw evidence](docs/BENCHMARKS.md).

The current engine does not implement file-content indexing, advanced tags, KIO/Baloo APIs, or a Dolphin search provider. Broad queries can have approximate rankings and longer latency tails. inotify typically requires a watch per directory, and some mounts are excluded by default.

**No controlled equivalent-feature comparison with Baloo has been run.** FerroSeek does not claim to be faster than, or a complete replacement for, Baloo. See [comparison context](docs/COMPARISONS.md) and [historical Baloo source research](docs/RESEARCH.md).

## Documentation

| Document | Purpose |
|---|---|
| [Installation](docs/INSTALLATION.md) | Components, setup, upgrade, rollback and archive details |
| [MCP](docs/MCP.md) | Agent tools, access modes and configuration |
| [Noctalia](docs/NOCTALIA.md) | Optional launcher plugin and CLI JSON contract |
| [Architecture](docs/ARCHITECTURE.md) | Core, index, watcher and optional adapters |
| [Benchmarks](docs/BENCHMARKS.md) | Reproducible performance results and caveats |
| [Comparisons](docs/COMPARISONS.md) | Where FerroSeek, Baloo, fd, rg and plocate fit |
| [Roadmap](docs/ROADMAP.md) | Implemented and planned capabilities |
| [Baloo research](docs/RESEARCH.md) | Original source analysis and references |
| [MCP discovery](docs/MCP_DISCOVERY.md) | Real-client tool-selection evaluation |

## Development

```sh
cargo build --release --locked --workspace
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

See [CLAUDE.md](CLAUDE.md) for repository conventions. FerroSeek is licensed under **GPL-3.0-or-later** ([LICENSE](LICENSE)). It includes no KDE source code; external KDE interfaces and Baloo research are credited in [the research notes](docs/RESEARCH.md). The current implementation has no telemetry or network indexing.
