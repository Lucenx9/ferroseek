# Initial MVP code review — 2026-10-31

Scope: the initial repository, empty tree
`4b825dc642cb6eb9a060e54bf8d69288fbee4904` to `0f17ed7`.
Standards and Spec reviews ran independently. Standards came from
CLAUDE.md and ARCHITECTURE.md; the specification was the owner's MVP
requirements and README, without an external issue tracker.

## Standards

Three findings, all corrected:

- **P1: Unbounded inotify read batches.** Reading until EAGAIN could grow
  one vector continuously before reaching the bounded channel. Reads now
  return after one buffer. A real inotify regression checks the per-call
  bound and delivery of every event across batches.
- **P1: Changed exclusions ignored after restart.** Unchanged directory
  mtimes bypassed new policy. A transactional policy key invalidates all
  directory listings when resolved rules or roots change. The regression
  excludes and re-includes an unchanged subtree across one-shot indexing
  and daemon startup. Shallower depth limits also discard stale children.
- **P2: Cache directories reappear through live events.** Event resolution
  now enforces CACHEDIR.TAG and rescans when the tag is removed or becomes
  invalid. A real-filesystem test covers suppression of new files and
  recovery of existing contents after tag removal.

## Spec

Three findings, all corrected:

- **P1: Slash-only queries panic.** Release panic=abort made this a daemon
  crash through KRunner Match("//"). These queries now return no results;
  both index tests and the release D-Bus smoke test cover the case.
- **P2: Path-only queries omit descendants.** The parser invented a filename
  requirement from the last directory component. Path-only queries now
  stream entries through full-path matching; `src/` finds `src/main.rs`.
- **P2: Candidate limits precede filters.** A matching file below `--in`
  could disappear behind thousands of unrelated matches. The budgets now
  count candidates after filtering. A regression places the only eligible
  file after 2100 unrelated matches.

## Verification and limits

Each of the six regressions failed before its corresponding fix and passed
afterward. Final checks: 66 workspace tests; cargo fmt check; Clippy with
`-D warnings`; locked release build; private-session D-Bus smoke test.
Tests used temporary directories. No KDE/Baloo configuration was changed,
and the installed desktop binaries were not replaced in this review.

Search still ranks a bounded candidate set, rather than promising globally
optimal ranking. Selective filters and path-only queries can read many rows
before filling that set; path caches are bounded separately. Their latency
needs new measurements. Existing BENCHMARKS.md results predate these fixes,
including the additional cache-tag checks during event handling. The first
startup with the new policy key revisits directory listings; unchanged
subsequent configurations retain the quick-reconcile optimization.

Standards: 3 findings resolved (worst P1). Spec: 3 findings resolved (worst P1).
