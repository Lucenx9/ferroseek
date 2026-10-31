# Release-readiness assessment — optimization pass, 2026-10-31

Scope: filename/path-search MVP on Linux/KDE Plasma; no content search,
semantic search, Dolphin search-provider API or Baloo compatibility claim.

## Changes and review

The path-query optimization reuses the current SQLite FTS5 and parent index.
No schema migration or extra persistent index is required. A focused review
caught broad-root expansion and prefix-ranking regressions in prototypes;
both were fixed and the latter has a permanent regression test. Native FTS
row-ID ordering avoids an unnecessary temporary sort. Search now uses one
read transaction, avoiding mixed snapshots during concurrent renames.

The resource/release review resulted in these additional fixes:

- Reader-pool epochs prevent in-flight queries from returning connections
  to a pool cleared by Teardown. Device/inode checks discard cached readers
  after the database file is replaced during recovery.
- Potentially blocking control commands and Teardown work run off the D-Bus
  async executor. Status no longer calls process-wide malloc_trim on every
  poll; the engine's idle release and session teardown still release memory.
- glibc arena limiting happens before threads start. Both allocator calls
  are gated to GNU libc; trimming is a no-op elsewhere. Their separate
  benefits are not isolated by this benchmark, and musl was not tested.
- CI builds the benchmark binary it actually invokes and includes installer
  regression tests. The declared Rust minimum is 1.89, matching locked
  dependency requirements, with a dedicated minimum-version job.
- Installer/uninstaller XDG fallback agrees with the application; malformed
  arguments fail early, uninstall reloads bus activation metadata, and
  repeated installs preserve configuration. Unsupported HOME characters
  are rejected before writes instead of silently corrupting Exec fields.
- Fresh benchmark runs reject nonempty work directories. Query-only mode
  validates its reuse flags before modifying anything. Legacy relevance
  percentages now exclude skipped samples from the denominator.

## Real Plasma verification

Tested on the available Plasma 6.8.2 session. The original FerroSeek install,
configuration and data paths were absent before testing. A new temporary
corpus was installed with a test-only systemd drop-in routing its data into
that same private test directory. Baloo configuration was not edited;
`Indexing-Enabled=false` remained present afterward.

- Restarted KRunner to discover the installed metadata, then queried the
  FerroSeek runner through KRunner's own `querySingleRunner` method.
  Its first query activated FerroSeek through the session bus/systemd.
  The journal recorded the real Match and a screenshot was inspected:
  the image appeared with the correct filename, image icon and parent path.
- The runner's Run action launched Gwenview with the test image. Its cgroup
  was a separate `app-org.kde.gwenview@…service` under `app.slice`, and it
  remained running across a FerroSeek service restart.
- Open-parent invoked the real FileManager1 path and created a new Dolphin
  process/window. The pre-existing Dolphin process was preserved.
- Create/rename/delete operations became searchable/disappeared as expected.
  During a 20,000-file full reconciliation, `scanning` was observed and 30
  Match calls completed; maximum external gdbus-process latency was 3.46 ms.
  This small desktop check is not a statistically controlled latency claim.
- Reinstalled while active, verified preserved configuration, then uninstalled
  twice. Only the newly created test configuration/drop-in was removed.
  No `--purge` was used and no user index was removed. The temporary data
  remains in its cache test directory. Test-created Gwenview/Dolphin processes
  were closed; the original Dolphin process remained alive.

The KRunner screenshot included the user's desktop and is intentionally not
committed. A focused private-bus smoke test also covers runner signatures,
URLs, relevance classes, concurrent calls, safe Run rejection, live changes,
pause/resume and clean termination.

## Scope and remaining limitations

- Broad/common path anchors and path components shorter than three characters
  may still use the original scanning path. Very large result sets retain
  the documented approximate candidate-budget ranking. Neither prototype
  regression is present in the accepted build.
- The ordinary mixed-query tail is a separate performance target from the
  selective-path gains; see BENCHMARKS.md for the actual rerun, not just the
  common-word subset.
- Installer binary paths currently support only ASCII letters, digits,
  underscore, dot, slash and hyphen. Arbitrary HOME-path quoting remains a
  packaging limitation, explicitly rejected rather than misinstalled.
- Cold whole-machine cache behavior, non-glibc targets and other Plasma/
  distribution combinations are not certified by this session. Advisory
  database eviction is labelled separately from warm-cache measurements.
- No packages, signatures, release publication or Baloo comparison were part
  of this task. Baloo remains disabled. The validated temporary installation
  was removed; no background service was left installed or enabled.

## Validation and release decision

Local checks passed: `cargo fmt --all --check`, workspace Clippy with
`-D warnings`, all **72** Rust unit/integration tests, workspace release
build, and `cargo +1.89.0 check --workspace --all-targets --locked`. Private
D-Bus smoke, installer regression and benchmark work-directory/reuse safety
tests passed. CI now runs the latter two scripts too; remote CI was not
executed in this local-only task.

The accepted production build improves selective 1M path-only queries from
529.820 to 2.032 ms and path+name from 109.939 to 2.069 ms, with five paired
process repetitions per size. Exact-name latency is unchanged. Idle RSS
after restart is 10.1 MiB; mixed-query p95 remains 24.80 ms. Broad `corpus/`
adds a small measured overhead (3.33 → 3.46 ms median). Repeated indexing
showed overlapping wall-time/write ranges: median 36.70 → 36.25 s and
1772.2 → 1707.7 MiB written at default throttle. A prior single-pass write
spike did not recur; its cause remains unproven. Initial/peak RSS varies
and the single full-run initial idle RSS increased 11.6 → 15.6 MiB.

**Assessment:** no known blocker to the scoped local Linux/glibc + Plasma
MVP on supported installer paths. No complete performance-target or
cross-platform release certification is claimed. For broader distribution,
remaining gates are testing additional distro/Plasma/libc combinations and
packaging/release delivery; arbitrary HOME-path support is still restricted.
Broad/short-path fallback costs and mixed-query tail latency are outstanding
performance work, not hidden by the selective-query improvements.

## Publication audit

Before the initial GitHub push, Gitleaks 8.30.1 scanned the complete `main`
history and an exported tracked tree with no findings. A separate scan of
all 207 historical file blobs checked credential patterns, personal local
paths, email addresses and binary artifacts. The only home paths found were
synthetic `/home/u` test fixtures. Commit attribution uses GitHub noreply
addresses. No user index, credential file, desktop screenshot or private
filesystem listing is tracked; benchmark evidence contains generated-workload
measurements and aggregate public `/usr/include` results.

The GPL-3.0-or-later manifest declaration, LICENSE, README, repository layout
and CI were reviewed. Ignore rules now also cover local environment files,
private-key files, SQLite databases, logs and Python caches. Formatting,
Clippy, all 72 Rust tests, release builds, installer/benchmark safety tests
and the private-bus smoke test passed again before publication. These scans
found no sensitive content; they are not a guarantee against every possible
secret format. Only `main` and its unchanged ancestors are intended for the
push; local orchestration/checkpoint refs are excluded. No release is created.

## Optional MCP validation milestone (2026-10-31)

The optional stdio MCP server now recovers live results behind more than 8,192
stale candidates. The [validation report](benchmarks/mcp-validation-2026-10-31/README.md)
records independent filename-search oracles, five-run tool comparisons,
private watcher measurements and rejected optimization evidence. All 108
Rust tests and local stable-1.99 Clippy/MSRV/release/smoke checks passed.

Remaining validation gaps are cold-cache behavior, more realistic source trees
and machines, long-run MCP workloads and completion of the separate desktop
soak. Broad scoped-query tails and approximate large-result ranking remain
known performance/behavior limitations. No release is published by this
milestone; the existing desktop installation and soak timer remain unchanged.


## Modular installation milestone

The newer installer supersedes the earlier HOME-character restriction and
implicit service restarts described above. It supports core/KRunner/MCP/full
selection, headless CLI installs, explicit agent registration, manifest-based
ownership and durable file rollback. Temporary-HOME tests include actual
private D-Bus activation from paths with Unicode, spaces and special characters.
All four local native archive variants have integrity and install/remove tests.
See [installation](INSTALLATION.md) for exact behavior and limitations.

Publication blockers for prebuilt binaries remain a signed/provenance release
workflow, a chosen libc baseline and a tested Linux distribution/architecture
matrix. Existing unmanaged installations with differing files require manual
migration; modified managed files are deliberately not overwritten. File
rollback cannot roll back external systemd state or synchronize agent clients'
concurrent config edits. No release is created by this milestone.
