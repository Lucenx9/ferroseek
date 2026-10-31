# FerroSeek roadmap

Each milestone counts as done only when it is implemented, tested and
measured. Compatibility claims require a verification against the real
consumer (Dolphin, KIO, the KF6 API), not just an interface that looks
compatible.

## M0 — MVP (done, 0.1)

- [x] SQLite FTS5 trigram index, chosen with a measured prototype
  (`prototype/backend-bench`).
- [x] Scanner and inotify watcher sharing one state-based reconciliation
  primitive:
  - rename pairing
  - stale-parent detection
  - overflow → reconcile
  - watch-limit degradation
- [x] Crash safety (WAL) and automatic recovery from a corrupt or
  incompatible index.
- [x] Pause/resume, duty-cycle throttling, nice 19 and idle I/O on the
  indexing thread, idle memory release.
- [x] CLI: `search`, `status`, `pause`, `resume`, `index`, `rebuild`,
  `config`.
- [x] KRunner D-Bus runner. Verified live on Plasma 6.8.2: discovery,
  activation, Match, Run, Open Containing Folder.
- [x] User-scoped install/uninstall, systemd user unit, D-Bus activation.
- [x] Benchmarks at 10k / 100k / 1M files (`docs/BENCHMARKS.md`).

## M1 — Hardening (next)

- [x] Optimize selective path queries with bounded directory scopes using
  existing indexes: five paired 1M runs, path-only 530 → 2.03 ms and
  path+name 110 → 2.07 ms (BENCHMARKS.md §9).
- [x] Correct skipped-sample relevance denominator; add independent path
  oracles, concurrency/ranking regressions and repeated benchmarks.
- [x] Audit reader lifecycle, allocator use, async control methods and
  idempotent install/uninstall; retest actual Plasma integration.
- [ ] Reduce mixed-query p95 (24.8 ms at 1M); improve broad/short path
  fallbacks without changing ranking or increasing persistent index size.
- [ ] Support additional HOME-path characters in installation templates.
- [x] Measure a real filesystem corpus (/usr/include, exact path-set oracle)
  and root-free advisory database eviction.

- [ ] Run the Baloo comparison protocol (`docs/BENCHMARKS.md` §6). This
  needs the owner's permission to enable Baloo temporarily.
- [ ] Extend real-workload coverage to varied home directories and
  whole-machine cold-cache conditions.
- [ ] Investigate the occasional ~0.4–0.7 s rename-visibility outliers seen
  at 1M files.
- [ ] Reduce the heap fragmentation left by the initial scan in the same
  process (idle anonymous memory is ≈ 4.8 MiB after the first index vs
  ≈ 3.1 MiB after a restart at 1M). Options: re-open the writer after the
  first pass, or allocator tuning.
- [ ] logind `PrepareForSleep` → a quick reconcile after resume. Today this
  is covered only by overflow detection plus the 6 h periodic pass.
- [ ] Reload the configuration on `SIGHUP` without a restart.
- [ ] Per-root status (watched or degraded) in `ferroseek status`.
- [ ] Optional `fanotify` backend (`FAN_REPORT_DFID_NAME`) where the
  privileges allow it, removing the one-watch-per-directory limit.

## M2 — Plain-text content search

- [ ] Opt-in `[content]` config: extensions (`txt`, `md`, `rst`, …), a
  maximum file size, and a maximum number of indexed bytes per file.
- [ ] A streaming reader (bounded buffer, never the whole file) with UTF-8
  validation and binary detection.
- [ ] A separate FTS5 table (unicode61 tokenizer) with an external-content
  design, so name-only users pay nothing.
- [ ] Re-extraction on `IN_CLOSE_WRITE` with a per-file backoff and a
  failed-files table (unlike Baloo's `failediddb`, give it a retry policy).
- [ ] Measure index growth, write volume and idle cost before enabling it
  by default.
- [ ] PDF and office formats stay optional and out of process until
  justified by measurements (Baloo bug 443547 shows the risk of in-process
  extractors).

## M3 — Desktop integration beyond KRunner

- [ ] A KIO worker or a `org.freedesktop.FileManager1`-style search
  provider for Dolphin's search bar. This needs a C++ or Qt shim only if the
  KIO API demands it; evaluate first.
- [ ] Recent/frequent-file boosting from the user's own activity, local
  only, no telemetry.
- [ ] A small KCM or settings page for roots and exclusions.

## M4 — Baloo API compatibility (research)

- [ ] Assess whether a compatible `Baloo::Query` backend, the
  `baloosearch:/` KIO protocol or the `org.kde.baloo` D-Bus interfaces can
  be served by FerroSeek without replacing KF6 libraries. Each would be a
  separate, opt-in component that is verified against the real clients.
  **No compatibility is claimed until then.**

## Non-goals

- Replacing KRunner or forking Plasma.
- Overwriting or disabling KDE components automatically.
- Network services or telemetry of any kind.
