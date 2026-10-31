# MCP tool discovery and selection

FerroSeek can make filename/path tools available to an agent; it cannot force
the agent to select them. Registration, client discovery, model tool retrieval
and invocation are different events. The fresh-session evaluation below
measures them separately. No installer adds global agent instructions.

## Metadata and official guidance

The stable server name remains `ferroseek`. Renaming it would disrupt existing
registrations without establishing a better tool choice. Initialization guidance
fits within 512 characters, placing filename/path discovery, cross-repository
scope, content-search exclusions and live-filesystem alternatives first.
Each tool description also stands alone when a client presents it without the
initialization instructions. Input descriptions explain literal substring
syntax, optional root narrowing and result limits.

- `search_files`: locate names and path fragments in authorized indexed data;
  avoids walking each repository. This describes its mechanism, not a measured
  speed advantage over filesystem tools.
- `get_file_info`: metadata/freshness of a known indexed path, not arbitrary
  stat or content reading.
- `index_status`: optional availability/authorization diagnosis. It does not
  list complete indexed coverage; totals and last write time are policy opt-ins.

Use `rg` for text inside files. `fd`, `rg --files`, Glob or direct filesystem
operations can be preferable for a known small directory, new/unindexed files
or an exhaustive live listing. No result from the index proves filesystem
absence. Hidden-name and authorization exclusions still apply, and updates
can lag. These descriptions do not broaden permissions or start indexing.

Sources checked on 2026-10-31:

- [MCP tools](https://modelcontextprotocol.io/specification/2026-07-28/server/tools)
  defines model-controlled tools, `tools/list`, descriptions and schemas;
  [initialization](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle)
  and current [server discovery](https://modelcontextprotocol.io/specification/2026-07-28/server/discover)
  expose server instructions. Neither guarantees model selection. The existing
  SDK already supports both flows; the offline test verifies identical metadata
  through each, without adding protocol handling to FerroSeek.
- [Claude Code MCP tool search](https://code.claude.com/docs/en/mcp#tool-search-for-mcp-server-authors)
  recommends task/category guidance and front-loaded terms. Current documentation
  describes 4,096-character startup descriptions/instructions and 16,384-character
  retrieved descriptions, not a universal 512-character protocol limit. We use
  the stricter 512-character budget requested for this project. We do not force
  eager loading with vendor-specific metadata.
- [Codex MCP configuration](https://learn.chatgpt.com/docs/extend/mcp?surface=cli)
  documents stdio registration, tool enablement, approval controls and required
  startup. The evaluation sets `required = true` so a missing server is a
  detectable setup failure rather than silent absence.

## Optional user guidance

Only if you want a durable preference, manually add this to your existing
`~/.claude/CLAUDE.md` and/or `$CODEX_HOME/AGENTS.md` (normally
`~/.codex/AGENTS.md`), preserving other instructions:

```text
For filename or partial-path discovery across projects, consider FerroSeek
search_files when available. It only covers authorized indexed paths. Use rg
for file contents and direct filesystem tools for new/unindexed files or
exhaustive live results. An empty index result does not prove absence.
```

[Claude's memory documentation](https://code.claude.com/docs/en/memory#claudemd-files)
and [Codex's AGENTS.md documentation](https://learn.chatgpt.com/docs/agent-configuration/agents-md)
explain these user-level locations. This guidance supplements registration;
it cannot connect a missing server, grant access or override higher-priority
client instructions. It is a preference, not a guarantee. Restart the client
or start a fresh session after configuration changes. The optional experiment
uses only temporary user profiles; real global instructions remain unchanged.

## Reproduction

The harness makes real authenticated model requests and can consume subscription
quota or API credit. It requires installed Claude Code/Codex and existing
file-based credentials. It copies only authentication files and configured
model/effort into private temporary profiles, removing them on normal completion.
It never publishes credentials. It does not copy user prompts, history, skills,
hooks or MCP registrations. Keep its raw output directory private.

Preserve the baseline executable built at `0618f0e`, then build this revision:

```sh
cargo build --release --workspace --locked
python3 scripts/benchmark-mcp-discovery.py \
  --baseline /absolute/path/to/baseline/ferroseek-mcp \
  --candidate target/release/ferroseek-mcp \
  --output bench-out/discovery-comparison --repetitions 3

# Separate optional-guidance experiment; not part of the default experience:
python3 scripts/benchmark-mcp-discovery.py \
  --baseline /absolute/path/to/baseline/ferroseek-mcp \
  --candidate target/release/ferroseek-mcp \
  --output bench-out/discovery-guidance --variants improved --guidance \
  --cases name multi_repo partial_path --repetitions 3
```

Each output directory must be new. `--clients claude` or `--clients codex`
limits the run. Each session times out after 90 seconds by default. The test
uses the user's configured model choice; different models/client versions can
produce different outcomes. Credential stores other than the two supported
files require separate setup in an isolated profile; no fallback model is used.

The corpus contains three small Git repositories with known files. All three
are indexed once. A file is then created without updating the index; another
lives in a non-indexed sibling directory. Every task has an independently
specified expected path set. Prompts never name FerroSeek, MCP or any tool:

| Case | Neutral request (synthetic root substituted at runtime) |
|---|---|
| name | Locate the file named orbit-config.toml under ROOT. Give its path. |
| multi_repo | Find every release-notes.md across the repositories under ROOT. Give their paths. |
| partial_path | Find the files whose path contains services/payments/ under ROOT. Give their paths. |
| content | Which Rust source files under ROOT contain RETRY_WINDOW_LIMIT? Give paths and matching lines. |
| recent | I just created fresh-memo.txt somewhere under ROOT. Locate it and give its path. |
| unindexed | Locate archive-map.json in ROOT/scratch. Give its path. |
| ambiguous | Locate README.md in the current repository and give its path. |

No conversations are resumed. Baseline/improved order reverses on alternate
repetitions. Both variants use the same frozen index, authorization, prompts,
model choice, built-in tools and read permissions. An observing stdio proxy
forwards messages unchanged and records MCP requests/responses. Claude runs
with non-persistent sessions, automatic memory disabled, only this MCP
registration, and read/search/shell tools permitted. Codex runs ephemeral
sessions with its read-only sandbox and an isolated configuration.

`results.json` in the private output contains candidate path matches and final
answers, not an automatic semantic grader. Review final answers against the
fixture, including invented paths, omissions, line contents and relative-path
interpretation. Do not equate a basename mention with successful completion.
Client JSON events record built-in calls; proxy events separately confirm
legacy initialization or modern server discovery, tool-list retrieval,
invocation and MCP round-trip latency.

## Measured outcomes (2026-10-31)

**The metadata change did not establish more reliable spontaneous selection.**
It clarifies appropriate use, corrects the status description and preserves
compatibility, but the default seven-case matrix selected built-in tools in
every session. Optional user guidance increased observed use in a separate
small experiment; it still did not make selection reliable.

114 scored fresh sessions: 84 primary (seven cases × three repetitions × two
clients × two metadata variants), 18 optional-guidance sessions, and 12
exploratory unknown-location sessions. Four earlier setup/pilot sessions are
excluded. Claude Code was 2.1.312, reporting `claude-sonnet-5-5`; Codex was
0.166.0, configured for `gpt-6-astra` with medium reasoning. Client defaults
otherwise remained intact. No model substitution or resumed context was used.

[Per-trial evidence](benchmarks/mcp-discovery/trials.json),
[summary data](benchmarks/mcp-discovery/summary.json), and
[exact wire metadata](benchmarks/mcp-discovery/metadata.json) are included.
The baseline source is `0618f0e`; candidate metadata is the revision containing
this report. Synthetic paths are replaced with `$CORPUS`; raw logs and temporary
credentials are not published.

### Availability, discovery and selection

Server negotiation and `tools/list` succeeded in **114/114** scored sessions:
Claude used modern `server/discover` (57 sessions), Codex used legacy
`initialize` (57 sessions).
This is client discovery, not proof that a model inspected or selected a tool.
Claude's explicit `ToolSearch` retrieval is separately recorded; Codex's direct
MCP invocations establish selection but do not expose a distinct retrieval
step. Absence of an explicit retrieval event does not prove model unawareness.

| Primary matrix | Baseline MCP selection | Improved MCP selection | Baseline task success | Improved task success |
|---|---:|---:|---:|---:|
| Claude Code | 0/21 (0%) | 0/21 (0%) | 20/21 (95.2%) | 21/21 (100%) |
| Codex | 0/21 (0%) | 0/21 (0%) | 21/21 (100%) | 21/21 (100%) |

All seven primary cases had zero MCP selections for both clients/variants in
all three repetitions. Claude used Glob/Grep/Bash; Codex used shell-based
`rg`. Those choices are suitable for this small corpus with an explicit search
root (or current repository). In particular, content, newly created and
unindexed-file requests correctly used live/content tools in **36/36** primary
sessions. This is not evidence that filename search beats content search.

One baseline Claude session returned `$CORPUS/config/orbit-config.toml` rather
than `$CORPUS/alpha/config/orbit-config.toml`, misinterpreting a relative Glob
result. It counts as task failure even though it found the basename. Other
answers matched the independent expected path sets; content answers also
reported the exact Rust source line. This isolated outcome is not evidence
that MCP metadata caused the success-rate difference: no MCP tool was invoked
in either primary group.

The optional experiment tested only the three discovery cases (name,
multiple repositories, partial path), three times each:

| Improved metadata, matched three-case subset | Without user guidance | With optional user guidance | Guided task success |
|---|---:|---:|---:|
| Claude Code | 0/9 (0%) | 3/9 (33.3%) | 9/9 |
| Codex | 0/9 (0%) | 4/9 (44.4%) | 9/9 |

All seven guided MCP calls were appropriate `search_files` queries. Claude
explicitly retrieved the tool in those three sessions. This preference is
useful evidence for documenting the opt-in snippet, not grounds to silently
install it or promise an agent will always follow it. Other task categories
were not rerun with guidance.

After seeing the primary preference for built-ins, we added an exploratory
request without a directory hint: “Find the path to retry-policy.yaml; I don't
remember which project contains it.” This is reported separately, not pooled
into the prespecified seven-case matrix:

| Unknown location (three repetitions) | Baseline selection | Improved selection | Explicit retrieval, baseline / improved | Task success, baseline / improved |
|---|---:|---:|---:|---:|
| Claude Code | 3/3 | 1/3 | 3/3 / 3/3 | 3/3 / 3/3 |
| Codex | 0/3 | 0/3 | not exposed separately | 3/3 / 3/3 |

Thus model-level discovery and actual invocation can diverge even when the
metadata was retrieved. The observed decrease for Claude is included rather
than hidden; three trials cannot establish a causal regression. No tuning on
this exploratory prompt or forced-use instructions were applied afterward.

Reproduce this separate check with the comparison command above and
`--cases unknown_location --repetitions 3`.

### Latency and additional calls

Elapsed task time includes CLI startup, authentication, model/network time,
tool discovery, tool execution and the final answer. These are descriptive
wall-clock observations, not a claim of a performance improvement. p95 uses
the sample at `round((n - 1) * .95)` in sorted observations.

| Primary matrix | Baseline task p50 / p95 (s) | Improved task p50 / p95 (s) | Baseline / improved tool calls |
|---|---:|---:|---:|
| Claude Code | 5.57 / 8.38 | 5.47 / 8.53 | 28 / 31 |
| Codex | 9.93 / 12.68 | 9.78 / 11.83 | 22 / 21 |

Primary MCP latency is **not applicable**, not zero: there were no MCP calls.
In the guided experiment, Claude task p50/p95 was 6.52/8.08 s, Codex
12.83/15.24 s. Proxy-observed MCP round-trip medians were 1.17 ms (three Claude
calls) and 1.13 ms (four Codex calls). Unknown-location medians and individual
calls are in the summary/trial data. The proxy measures forwarding a request
to receiving its response, including server processing and stdio; it excludes
host-side tool preparation and model time. The sample is too small to infer
server tail latency or compare clients' transport performance.

A manual conservative redundancy review counted **six duplicate lookup calls**:
three in guided Claude sessions, two in baseline unknown-location sessions,
and one in an improved unknown-location session. Each repeated an equivalent lookup within the session, with matching result
sets and at least one lookup sufficient to supply the complete paths. This
includes overlapping/parallel confirmation work, not only sequential retries. Two other improved unknown-location sessions retrieved the MCP tool
but never invoked it (avoidable retrieval overhead, counted separately).
We do not label all second calls unnecessary: resolving ambiguous relative
Glob output or widening a content search to hidden/ignored files can be
justified. No `index_status` or `get_file_info` prerequisite calls were made.

### Limits, review and validation

This small synthetic workload tests selection and correctness, not realistic
index scale. Explicit root hints make filesystem tools reasonable choices.
There are only three repetitions per case; no statistical significance or
population-level selection-rate claim is made. Optional-guidance and
exploratory batches ran alongside parts of the primary batch, so network and
machine load affect task timing. The corpus used tmpfs with warm filesystem
caches, a deliberately frozen index, default restricted authorization for its
common parent, and no freshness watcher. This does not measure the installed
desktop daemon's behavior or global-mode discovery.

A release rebuild occurred early in measurement during full-workspace checks.
The observer verified identical instructions/tool-schema hashes for every
session within each metadata variant, including the pilot; no search logic or
policy changed. Future harness runs freeze copies of both executables before
starting sessions. Published evidence is identified by wire-metadata hashes;
these observations must not be used for binary performance attribution.

Review covered truthful capabilities, scope/freshness caveats, stable names and
schemas, protocol output visibility, benchmark isolation and credential/report
hygiene. Scope is metadata, tests and documentation; engine, schema, daemon,
KRunner and access policy are unchanged. No live service or global agent
configuration was modified. The reliable-default-selection goal remains
unproven; registration alone is insufficient in these clients and conditions.

Validation: 112 Rust workspace tests, formatting, Clippy with warnings denied,
release workspace builds, Rust 1.89 compatibility, installer/archive/benchmark/
index-oracle smoke tests, all supported MCP protocol smoke tests and private
D-Bus integration checks passed. The new offline discovery-harness smoke test
checks neutral prompts, independent fixture expectations, frozen-index misses,
and transparent protocol forwarding without authenticating any model; it runs
in CI. Paid model evaluations are deliberately not part of CI.
