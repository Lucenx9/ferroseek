# Search tools, comparisons and background

FerroSeek began as an investigation into **Baloo**, but it is an independent
Linux filename/path indexer, not a Baloo fork or a drop-in replacement for
KDE's search framework. This page summarizes where different tools fit;
it is **not** a performance ranking.

| Tool | Main use | Important distinction |
|---|---|---|
| **FerroSeek** | Indexed filename/path discovery across configured Linux directories | Incremental index with optional CLI, KRunner and MCP; currently no file-content search |
| **Baloo** | KDE file indexing, search integration and metadata/content features | Exposes KDE capabilities and interfaces FerroSeek does not currently implement |
| **fd** | Filename/path discovery in the live filesystem | Does not require a persistent index for each search |
| **ripgrep (`rg`)** | Text/regex **inside files**, or filename enumeration with `rg --files` | Content search is not equivalent to FerroSeek's current filename-only search |
| **plocate** | Indexed pathname lookup | Has its own update model and a different set of metadata/search features |

## Published evidence

- [Core benchmarks](BENCHMARKS.md) cover filename indexing, idle memory,
  query latency, recovery, datasets and hardware with stated limitations.
- [MCP filename-tool comparisons](benchmarks/mcp-validation-2026-10-31/README.md)
  measure specific end-to-end filename/path workflows against `fd`,
  `rg --files` and `plocate` with an independent filesystem oracle.
  Invocation patterns and index-maintenance costs differ.
- [MCP discovery evaluation](MCP_DISCOVERY.md) distinguishes registration,
  client discovery and actual agent tool selection. Agents can reasonably
  choose built-in tools for small directories even with MCP available.
- [Baloo architecture research](RESEARCH.md) records historical source
  analysis, referenced KDE issues and the project's original motivation.

## What is not established

**No controlled, equivalent-feature Baloo comparison has been performed.**
FerroSeek does not currently offer all of Baloo's file-content, metadata,
tag and KIO search functionality. Filename benchmarks must not be presented
as overall performance or feature-parity claims.

An index has initial and ongoing maintenance costs, covers only configured
roots and may briefly lag changes on disk. Direct filesystem tools can be
better suited to a small known repository, regex/content searches, or files
created outside the index.

Future comparisons should report the dataset, query semantics, cache state,
freshness, permissions, indexing cost, client invocation, correctness and
memory/CPU/I/O measurements rather than a single speedup number.
