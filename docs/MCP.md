# FerroSeek MCP server (optional)

`ferroseek-mcp` lets MCP-compatible agents such as Claude Code and Codex
search FerroSeek's **existing** file-name index.

- **Optional.** It is a separate binary that only runs while an agent
  starts it, over stdio, and exits when the agent disconnects. When no
  agent is running it, it uses no CPU or memory.
- **Read-only.** It opens the daemon's SQLite database with a read-only
  `Reader`. It starts no indexer, writes nothing and keeps no copy of the
  index. The daemon, KRunner and the index schema are unchanged.
- **Local only.** It uses the stdio transport only: no HTTP, no listening
  socket, no network code, no telemetry.
- **Names and metadata only.** There is no content search, semantic search,
  embeddings, or file reading.

## Tools

| tool | input | output |
|---|---|---|
| `search_files` | `query` (required), `root` (optional absolute dir), `limit` (optional) | `results[]` (`path`, `kind`, `size`, `modified`, `non_utf8`), `more_available`, `truncated_by_size`, `omitted_stale`, `incomplete_due_to_changes` |
| `get_file_info` | `path` (absolute) | `path`, `kind`, `size`, `modified`, `exists`, `up_to_date` |
| `index_status` | — | `available`, `error`, `allowed_dirs[]` (`path`, `indexed`), `max_results`. With `expose_index_totals`: whole-index totals, size, `last_write` |

All three are annotated `readOnlyHint: true` and `openWorldHint: false`, and
return structured content with a declared `outputSchema`. The SDK also
mirrors the content as text for older clients.

The query syntax is FerroSeek's: all words must match as case-insensitive
substrings of the name, and a word containing `/` must match the full path.

## Access control

**Being indexed does not authorize access.** An agent sees a path only when
all of the following hold:

1. **It is strictly below an explicitly allowed directory.** Allowed
   directories come from `--allow DIR` and/or `allow = [...]` in
   `~/.config/ferroseek/mcp.toml`. With no allowed directories, every tool
   refuses every path.
2. **No component below that directory is a dotfile or dot-directory**,
   unless `allow_hidden = true`.
3. **No component below it matches a deny pattern.** Matching is ASCII
   case-insensitive, so `CREDENTIALS/` and `server.PEM` are caught too. The
   built-in list
   covers `.ssh`, `.gnupg`, `.password-store`, `.aws`, `.kube`, `.docker`,
   `.netrc`, `.env`/`*.env`, `id_rsa*`/`id_ed25519*`…, `*.pem`, `*.key`,
   `*.p12`, `*.kdbx`, `*secret*`, `*credential*` and similar (see
   `crates/mcp/src/policy.rs`). `deny = [...]` adds patterns, and
   `deny_defaults = false` drops the built-in list. (Three of these four
   points come from the independent review; see below.)

Path handling:

- Client paths must be absolute and canonical. Any `.`, `..`, empty
  component or NUL byte is refused, never normalized.
- Containment is checked by path component, so `/home/u/Progetti` does not
  authorize `/home/u/ProgettiX`.
- Error messages never echo a refused path.
- `"/"` cannot be allowed.

Containment is lexical and matches what the index stores, because the index
never follows symlinks. A symlink in an allowed directory is reported as a
`symlink` entry. Its target is not searched unless the target is itself
indexed *and* allowed.

Disk metadata (for freshness and staleness) is read **without following
symlinks at any level below the allowed directory**. Each path component is
opened with `openat(O_PATH|O_DIRECTORY|O_NOFOLLOW)` relative to the
previous one, then `fstatat(AT_SYMLINK_NOFOLLOW)`
(`crates/mcp/src/beneath.rs`). If a directory was replaced by a symlink
after indexing, the entry counts as stale; the metadata of whatever the
symlink points to is never reported.

The access policy runs *inside* the index search, through the optional
`SearchOptions::accept` predicate. It is applied where the index applies
its own `within`/hidden filters, before its candidate budget. Hidden,
denied or out-of-scope entries therefore never use up that budget, however
many there are, and `more_available` reflects only authorized, existing
results. If deleted entries crowd out live results, the request retries with
filesystem validation inside the candidate budget too. A stable deleted
prefix longer than 8192 entries no longer hides live matches. This fallback
uses the same no-symlink metadata checks and only touches authorized paths.

Concurrent changes between candidate validation and response construction can
still require larger fetches, bounded at 8192 per scope. If that retry budget
is exhausted, `incomplete_due_to_changes` is true: refine or retry the query.
It is not a general filesystem snapshot/race detector. `omitted_stale` is a
lower bound on discarded stale matches, not a full count. Existing index
candidate limits still make ranking approximate for very broad queries.

`index_status` never reports the database path. Whole-index totals, size and
last write time describe unauthorized directories too, so they appear only
with `expose_index_totals = true`. Error messages are generic, and the
details (including the path) go to stderr only, which clients keep in local
logs.

Searches run per allowed directory. Unauthorized entries are filtered from
the candidate set, and every result is checked against the policy before it
leaves the process. `index_status` reports index totals as numbers but lists
only the allowed directories, never the other indexed roots.

### `mcp.toml`

```toml
# ~/.config/ferroseek/mcp.toml  (all keys optional; unknown keys are errors)
allow = ["~/Progetti", "~/Documenti/lavoro"]
deny = ["*.sqlite", "private-*"]   # added to the built-in deny list
deny_defaults = true               # set false to drop the built-in list
allow_hidden = false               # dotfiles below allowed dirs
max_results = 50                   # default 50, hard cap 200
expose_index_totals = false        # whole-index totals/size/last write in index_status
```

`--allow` arguments are added to `allow`. `--config FILE` selects another
policy file, and `--index FILE` another database. The default database is
the daemon's, `$XDG_DATA_HOME/ferroseek/index.sqlite`, which also honours
`FERROSEEK_DATA_DIR`.

## Behaviour under real conditions

- **The index is missing or the daemon was never run.** `index_status`
  returns `available: false` with a generic reason, and the other tools
  return a tool error (`isError: true`) explaining how to create the index. Once the
  index exists, the next call works without restarting the MCP server.
- **The daemon is writing.** SQLite WAL lets readers proceed while it
  commits, and queries see the last committed state. This is tested with
  continuous writes during queries.
- **The database is replaced** (the daemon's corruption recovery). The
  cached connection is keyed by device and inode and reopened when they
  change.
- **Stale entries.** Every search result is checked on disk without
  following symlinks (see above). Entries deleted since the last index
  update, or whose ancestors changed, are dropped and counted in
  `omitted_stale`; size and mtime come from the disk. `get_file_info` reports `exists` and
  `up_to_date` (whether the index already reflects the file).
- **Invalid input.** Empty, overlong (more than 512 bytes) or
  control-character queries, malformed paths and unknown tools return clean
  errors, and the server keeps serving.
- **Bounded responses.**
  - `limit` defaults to 20 and is capped by `max_results` (default 50,
    hard cap 200).
  - The serialized result list is cut at 24 KiB (`truncated_by_size`).
    Because of the text mirror, the full JSON-RPC message is at most about
    48 KiB.
  - At most 32 allowed directories.

## Installation

Build it (or use the workspace release build) and install it next to the
other binaries:

```sh
cargo build --release -p ferroseek-mcp
install -Dm755 target/release/ferroseek-mcp ~/.local/bin/ferroseek-mcp
```

To uninstall, run `rm ~/.local/bin/ferroseek-mcp`, remove the client
registration below, and optionally delete `~/.config/ferroseek/mcp.toml`.
`scripts/install.sh` and `scripts/uninstall.sh` do not install or remove the
MCP server; it is opt-in. The daemon must be running, or `ferroseek index`
must have been run, for there to be an index to query.

### Claude Code

```sh
# Available in all your projects:
claude mcp add --scope user ferroseek -- ~/.local/bin/ferroseek-mcp --allow ~/Progetti
claude mcp list            # should show ferroseek ✓ connected
```

Or in a project's `.mcp.json`:

```json
{
  "mcpServers": {
    "ferroseek": {
      "type": "stdio",
      "command": "/home/YOU/.local/bin/ferroseek-mcp",
      "args": ["--allow", "/home/YOU/Progetti"]
    }
  }
}
```

### Codex

```sh
codex mcp add ferroseek -- ~/.local/bin/ferroseek-mcp --allow ~/Progetti
```

Or in `~/.codex/config.toml`:

```toml
[mcp_servers.ferroseek]
command = "/home/YOU/.local/bin/ferroseek-mcp"
args = ["--allow", "/home/YOU/Progetti"]
```

Prefer listing allowed directories in `mcp.toml`: one policy then applies to
every client.

## Verification performed (2026-10-31)

- **Unit tests** (`crates/mcp/src/policy.rs`, 8): path canonicality,
  component-wise containment, hidden/deny rules below the allowed
  directory, config parsing, scopes, non-UTF-8 names, and a property test
  that the compiled deny patterns match exactly like the glob matcher.
- **Unit tests** for the race-safe metadata walk (`crates/mcp/src/beneath.rs`,
  2).
- **Protocol tests** (`crates/mcp/tests/protocol.rs`, 18). They run the
  real binary over stdio against indexes built by the real engine:
  - tool listing with schemas and annotations
  - allowlist confinement, traversal refusal without echoing the path
  - secret, hidden and stale filtering
  - limits and the byte budget
  - invalid input, no allowlist
  - a missing index that later appears
  - queries during continuous index writes
  - exit on EOF, multi-root confinement
  - every declared `required` output property present in real responses
- **Protocol smoke** (`tests/mcp-smoke.py`): clients on protocol versions
  2024-11-05, 2025-03-26, 2025-06-18 and 2025-11-25 each negotiate their own
  version, with nothing printed on stderr.
- **Real clients, temporary synthetic data, no configuration changed:**
  - **Claude Code 2.1.312** (`claude -p --mcp-config … --strict-mcp-config`):
    connected, and all three tools worked. A file outside the allowlist was
    refused, and `.secrets/*.key` and `*.pem` never appeared. The first run
    found a bug (`non_utf8` was marked required in the output schema but
    omitted when false), fixed with a regression test.
  - **Codex CLI 0.166.0** (`codex exec -c mcp_servers.ferroseek…`, read-only
    sandbox): three MCP calls completed, with no shell use.

## Independent review (GPT 6 Astra, high effort, 2026-10-31)

A read-only review of commit `364ac15` reproduced four defects, all fixed
with regression tests:

| severity | finding | fix and test |
|---|---|---|
| high | `lstat` followed a symlinked *ancestor*: after indexing `allowed/sub/record.txt`, replacing `sub` with a symlink exposed the metadata of an unauthorized file | `stat_beneath` openat walk; `symlinked_ancestor_never_reveals_outside_metadata`, `beneath::tests` |
| medium | `more_available` was computed before filtering (an oracle for hidden or denied names), and filtered candidates could starve visible ones | policy runs inside the index budget (`SearchOptions::accept`); `more_available_is_not_an_oracle…`, `filtered_candidates_do_not_starve…`, `more_denied_candidates_than_any_cap…` |
| medium | `index_status` and errors exposed the database path and whole-index statistics | generic errors, path never reported, totals opt-in; updated status and missing-index tests |
| medium | the deny list was case-sensitive (`CREDENTIALS/`, `server.PEM`) | ASCII case-insensitive matching; policy unit tests |

The reviewer also confirmed the following as correct: the `rmcp` 3.8 API
usage and output schemas, the stdio-only and read-only design, containment
and traversal handling, the unscoped fast path, and the byte and count
bounds.

**Re-check of the fixes (same model, a new session).**
- Findings 1, 3 and 4 were confirmed **closed**.
- Against the original commit, five of the new regression tests failed as
  expected.
- 10,000 stat calls showed no file-descriptor growth.
- Finding 2 was only **partly** closed by the first fix, an MCP-side
  candidate cap. 8200 denied, higher-ranked matches still starved a
  visible result and produced a false `more_available: false`. It is now
  fixed at the root by moving the policy into the index budget, and that
  exact scenario is a test.

**Third round (same model, a new session).** It confirmed **finding 2
closed**, using a filesystem-computed oracle with 10,500 denied and hidden
entries, ties, scoped and unscoped searches, short tokens, `/` tokens and
stale entries. It also checked the compiled deny patterns exhaustively
(1,365 patterns × 781 names). It found two new medium issues introduced by
the latency optimization, both now fixed:

- **Ranking.** Truncating the name ranking before the depth and recency
  adjustments could drop the best *final* result. It is replaced by an
  exact top-k early stop (stop once `name score + 6` falls below the k-th
  best final score). Test: `accept_only_searches_return_the_true_top_k`,
  which fails with the old truncation.
- **Memory.** The per-search memo of parent verdicts is now bounded (4096
  directories).

The index-side change is opt-in (`accept: None` everywhere else). The
daemon's own benchmark showed no regression for KRunner `Match` at 100k
(p50 0.66 / p99 3.36 ms against 0.77 / 3.30 ms before), with relevance
still at 100 %.

## Performance

These are warm-cache measurements on the 100k and 1M synthetic benchmark
indexes. Details, commands and raw reports are in
[BENCHMARKS.md §10](BENCHMARKS.md#10-mcp-server-2026-10-31).

| at 1M entries | |
|---|---|
| startup to `initialize` reply | 1.1 ms |
| `search_files`, limit 20: p50 / p95 / p99 | 4.81 / 12.08 / 15.74 ms |
| `get_file_info` p50 | 0.09 ms |
| `index_status` p50 (default, no totals) | 0.06 ms |
| RSS after startup | 5.1 MiB (0.4 MiB anonymous) |
| idle RSS after a 650-request workload | 11.4 MiB (4.8 MiB anonymous) |

Enforcing the policy inside the index budget costs about 1.6 ms at the 1M
median compared with the earlier, starvation-prone version (3.22 ms).
Every candidate's parent directory has to be resolved. This was traded
deliberately for correctness.
| idle CPU / wakeups | 0.000 % / 0 per s |

## Limitations

- **`root` filters on a subdirectory are slower** (1M: p50 9.1 ms, p99
  42 ms). They use the index's subtree scope. Allowed directories that
  contain every index root take a faster unscoped path.
- **With `expose_index_totals`, `index_status` counts the whole index**
  (about 135 ms at 1M), and the totals include unauthorized directories.
- **Results reflect the last committed index state.** A file created a
  moment ago appears after the daemon's debounce (about 200 ms).
- **Non-UTF-8 paths are returned lossily** and flagged `non_utf8: true`, so
  the agent cannot reconstruct the exact bytes.
- **Allowed directories must be given as the index stores them.** A symlink
  pointing into an indexed tree does not match; `index_status` then shows
  `indexed: false`.
- **Matching is name-based, not semantic.** File contents are never read or
  returned.
