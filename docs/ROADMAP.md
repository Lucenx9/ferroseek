# FerroSeek roadmap

FerroSeek is a **desktop-independent Linux filename/path search engine** with optional terminal, desktop and agent interfaces. Checked items reflect implemented and tested work; they do **not** mean a public binary release or cross-distribution certification is complete.

## Implemented in the 0.1 MVP

- [x] Rust search core, SQLite FTS5 trigram filename index, parent-pointer tree and ranked path/name queries.
- [x] Inotify-driven incremental indexing, reconciliations, recovery, background throttling and pause/resume.
- [x] CLI for filename/path search, index maintenance and status, including headless/one-shot use.
- [x] Optional KRunner D-Bus adapter tested in a KDE Plasma 6 session.
- [x] Optional, read-only MCP stdio server using the shared index; real Claude Code/Codex protocol tests.
- [x] MCP `restricted`, `home` and `all_indexed` authorization modes, default sensitive-name rules and bounded results.
- [x] User-scoped modular installer, dry-run, owned-file checks, rollback, local archive generation and CI smoke tests.
- [x] Benchmark harnesses and independently checked search/correctness test fixtures.

See [architecture](ARCHITECTURE.md), [installation](INSTALLATION.md), [MCP](MCP.md) and [benchmarks](BENCHMARKS.md).

## Near term — hardening and distribution

- [ ] Finish extended real-world stability checks across larger and more varied user directories.
- [ ] Validate additional Linux distributions, architectures, libc environments and desktop configurations.
- [ ] Decide binary libc baselines; add release provenance, SBOM/dependency notices, signing/checksums and reproducible release builds.
- [ ] Publish a scoped first beta after packaging and functional validation.
- [ ] Strengthen stress tests for forced inotify overflow, permission failures, suspend/resume, races and interrupted reconciliation.
- [ ] Improve per-root watch/degradation reporting, and investigate occasional rename-visibility outliers at scale.
- [ ] Keep source installation and uninstall safe; improve diagnostics without automatically modifying running installations.

## Search and indexing

- [ ] Improve broad/short-path fallback latency and the mixed-query p95 while preserving correctness and low memory use.
- [ ] Consider indexed or query-time metadata filters for type, extension, size and modification date, measured against the existing baseline.
- [ ] Add **optional** bounded plain-text content indexing for TXT/Markdown/source files with separate performance and size benchmarks.
- [ ] Keep potential PDF/office extractors isolated and optional until justified by measurements.
- [ ] Investigate initial-scan heap retention, dedicated resume reconciliation, SIGHUP config reload and alternate watch strategies only where supported by evidence.

## Desktop and terminal integrations

- [x] Core CLI and KDE KRunner.
- [x] Optional Noctalia v5 `/ferro` provider and versioned CLI JSON output; isolated CLI/mock-host tests. See [setup and remaining desktop validation](NOCTALIA.md).
- [ ] Rofi adapter: validate real query/update capabilities rather than assuming arbitrary per-keystroke search.
- [ ] GNOME Shell SearchProvider2 adapter, tested under a real GNOME session.
- [ ] Evaluate bounded dmenu/Fuzzel/Wofi workflows and Walker/Elephant integration.
- [ ] Consider Dolphin/KIO search integration and a KDE settings page as optional adapters; neither is a core dependency.

All desktop adapters should reuse the same index and avoid a permanent extra background service when not needed.

## AI agent integration

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

- Rebuilding KRunner, replacing KDE Plasma or silently disabling other search engines.
- Mandatory agent tooling, embeddings, remote indexing services or telemetry.
- Claiming feature parity or universal platform support before verification.
