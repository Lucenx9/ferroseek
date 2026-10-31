# MCP server measurements (2026-10-31)

These are raw outputs of `scripts/benchmark-mcp.py`, run against the release
`ferroseek-mcp` binary listed in `SHA256SUMS`. It was built from the working
tree on top of `bc8b917`; the MCP commit records the exact source. The
indexes are the generated benchmark corpora (seed 42) from
`ferroseek-bench`, opened read-only. The machine and toolchain are as in
BENCHMARKS.md §1.

```sh
cargo build --release -p ferroseek-mcp
scripts/benchmark-mcp.py --bin target/release/ferroseek-mcp \
  --index ~/.cache/ferroseek-bench-100000/data/index.sqlite \
  --allow ~/.cache/ferroseek-bench-100000/corpus > report-100k.md
scripts/benchmark-mcp.py --bin target/release/ferroseek-mcp \
  --index ~/.cache/ferroseek-bench-1000000/data/index.sqlite \
  --allow ~/.cache/ferroseek-bench-1000000/corpus > report-1m.md
```

There is one run per size, with a warm page cache. The ferroseek daemon was
not involved; the live desktop daemon kept running independently, on its
own small index.
