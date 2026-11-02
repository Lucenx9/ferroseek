# FerroSeek roadmap

FerroSeek is a **keyboard-first Linux launcher** for applications, files and
calculations, powered by its own Rust file search engine. The standalone
launcher is the main product. The engine and CLI remain independently usable;
MCP remains an optional read-only component. Desktop launcher adapters have
been removed.

The launcher is experimental. Checked items describe implemented work, not a
published release or verified compatibility with every Linux desktop.

## Main priority: a launcher for daily use

- [x] Wayland layer-shell UI with application search, indexed file results,
  calculator, local launch history and keyboard actions.
- [x] Single-instance toggle, private mode, file opening and reveal-in-folder.
- [x] Managed `--launcher` installation/removal and launcher archives including
  the engine and CLI; guided setup recommends the launcher on Wayland.
- [ ] Validate real desktop sessions and publish an explicit compositor support
  matrix, including multiple monitors, display scaling and keyboard layouts.
  Current graphical evidence is nested virtual KWin only.
- [ ] Make focus and application activation reliable, including XDG activation
  tokens where supported.
- [x] Dedicated FerroSeek daemon search/open interface, independent replies for
  concurrent clients, and removal of Noctalia/KRunner adapters.
- [x] Choose a compatible installed application for an indexed file without
  changing defaults; preserve the selected file through asynchronous updates.
- [ ] Add file details and bounded previews for selected supported formats with
  explicit loading, unsupported and error states.
- [ ] Navigate into folders and back without losing the previous query,
  search scope or selection.
- [ ] Add configuration for keyboard shortcuts, appearance, light/dark themes,
  size and position. Keep focus, navigation and feedback consistent across views.
- [x] Initial folder selection, folder editing with save/apply, file-search status,
  indexed locations, service errors and explicit refresh/retry in the launcher.
- [ ] Measure startup and typing-to-results latency with the UI and daemon
  together before choosing further performance work.
- [ ] Publish a scoped launcher beta after desktop and packaging validation.

Delivery order: verify desktop focus/activation and measure end-to-end latency;
then file details and Open with; folder navigation; quicklinks and keyword
commands. Each interaction change needs keyboard, pointer, long-label and
asynchronous/error checks, plus inspection of rendered UI.

See [launcher setup and current limits](LAUNCHER.md).

## Existing search engine and supporting components

- [x] Rust search core, SQLite FTS5 trigram filename index, parent-pointer tree and ranked path/name queries.
- [x] Inotify-driven incremental indexing, reconciliations, recovery, background throttling and pause/resume.
- [x] CLI for filename/path search, index maintenance and status, including headless/one-shot use.
- [x] Optional, read-only MCP stdio server using the shared index; real Claude Code/Codex protocol tests.
- [x] MCP `restricted`, `home` and `all_indexed` authorization modes, default sensitive-name rules and bounded results.
- [x] User-scoped modular installer, dry-run, owned-file checks, rollback, local archive generation and CI smoke tests.
- [x] Benchmark harnesses and independently checked search/correctness test fixtures.

See [architecture](ARCHITECTURE.md), [installation](INSTALLATION.md), [MCP](MCP.md) and [benchmarks](BENCHMARKS.md).

## Near term — hardening and distribution

- [ ] Finish extended real-world stability checks across larger and more varied user directories.
- [ ] Validate additional Linux distributions, architectures, libc environments and desktop configurations.
- [ ] Decide binary libc baselines; add release provenance, SBOM/dependency notices, signing/checksums and reproducible release builds.
- [ ] Strengthen stress tests for forced inotify overflow, permission failures, suspend/resume, races and interrupted reconciliation.
- [ ] Improve per-root watch/degradation reporting, and investigate occasional rename-visibility outliers at scale.
- [ ] Keep source installation and uninstall safe; improve diagnostics without automatically modifying running installations.

## Search engine improvements

- [ ] Improve broad/short-path fallback latency and the mixed-query p95 while preserving correctness and low memory use.
- [ ] Evaluate bounded typo tolerance for filename queries against relevance
  fixtures and latency measurements. Existing application fuzzy matching does
  not establish typo tolerance for indexed file search.
- [ ] Consider indexed or query-time metadata filters for type, extension, size and modification date, measured against the existing baseline.
- [ ] Add **optional** bounded plain-text content indexing for TXT/Markdown/source files with separate performance and size benchmarks.
- [ ] Keep potential PDF/office extractors isolated and optional until justified by measurements.
- [ ] Investigate initial-scan heap retention, dedicated resume reconciliation, SIGHUP config reload and alternate watch strategies only where supported by evidence.

## Daily commands and later extensions

These are planned capabilities, not available launcher features. Complete the
file/application workflow and desktop validation before expanding the platform.

- [ ] Add user-defined aliases, quicklinks to URLs/folders and keyword-based web
  searches, with editable targets and predictable argument handling.
- [ ] Add searchable clipboard history and reusable text snippets as opt-in
  features, with clear retention/deletion controls and explicit desktop support.
- [ ] Evaluate user-defined script commands before designing workflows or a
  general extension API; keep execution explicit and separate from search.

## Optional AI agent integration

- [x] MCP stdio `search_files`, `get_file_info`, `index_status` with read-only index access.
- [x] Restricted and optional global indexed-root modes, hidden/sensitive-name filtering, and live existence checks.
- [x] Protocol tests and independent fresh-session agent discovery experiments.
- [ ] Investigate slow tail queries scoped to large subtrees and exact encoding of non-UTF-8 result paths.
- [ ] Measure real agent task time, tool calls and token overhead in realistic cross-project workflows. MCP discovery does not guarantee selection.
- [ ] Consider additional safe metadata filters as the core gains them; no automatic arbitrary content access.

## Comparisons and optional compatibility

- [ ] Run a **controlled equivalent-feature Baloo comparison** in an explicitly authorized or isolated setup. None has been completed yet.
- [ ] Maintain caveated comparisons with `fd`, `rg --files` and `plocate`; content-search comparisons must use equivalent features.
- [ ] Research optional KF6/KIO compatibility separately. FerroSeek currently offers neither `Baloo::Query` nor `baloosearch:/` nor full Dolphin API compatibility.

See [comparisons](COMPARISONS.md), [source-level Baloo research](RESEARCH.md) and [MCP discovery evidence](MCP_DISCOVERY.md).

## Non-goals

- Replacing KDE Plasma or silently disabling other search engines.
- Expanding into a general plugin platform or clipboard manager before the
  application/file launcher is reliable for daily use.
- Mandatory agent tooling, embeddings, remote indexing services or telemetry.
- Claiming feature parity or universal platform support before verification.
