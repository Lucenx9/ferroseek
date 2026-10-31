# backend-bench — index backend prototype

Throwaway prototype (kept as a primary source) answering one question:
**which index backend fits FerroSeek's filename workload?** The workload is
case-insensitive substring search over file names, plus frequent small
durable updates from inotify, with low idle memory as the top priority.

## Method

`src/main.rs` builds the same deterministic synthetic corpus (seeded
xorshift, names like `IMG_20240412_000123.jpg`, `budget-final-42.pdf`,
`ReportSummary.md`) into four backends, each in a **separate process**:

| backend | substring strategy | update strategy |
|---|---|---|
| `sqlite` | FTS5 `trigram` tokenizer, contentless + `contentless_delete=1`, plus `entries` table | one transaction per insert+delete |
| `tantivy` 0.27 | `NgramTokenizer(3,3)` + lowercase, AND of trigram terms, then verify | `add_document` + `delete_term` + `commit` |
| `fst` 0.4 | mmap'd FST map with a KMP "contains" automaton (no pruning possible) | full rebuild + fsync + rename (FSTs are immutable) |
| `scan` | fsearch-style: names in RAM, `str::contains` per name | rewrite flat file + fsync + rename |

The queries are 12 fixed substrings from very selective to unselective. The
result count is capped at 2000, the way a ranking engine would bound its
candidate set. Each query runs 20 times after a warm-up. "Update" means one
durable insert plus one delete.

```sh
cargo build --release
./target/release/backend-bench <sqlite|tantivy|fst|scan> <N> <dir-on-real-disk>
SQLITE_SYNC=FULL ./target/release/backend-bench sqlite <N> <dir>   # fsync per commit
```

## Results (2026-10-31)

The machine was a 16-thread x86_64 with 31 GiB RAM and NVMe running Btrfs,
on CachyOS kernel 7.3.0, rustc 1.94.0, bundled SQLite 3.54.1 and
Tantivy 0.27.0. Raw output is in `results-btrfs.txt`. The first attempt ran
in `/tmp`, which is **tmpfs** here, so it made fsync free. That run was
discarded.

At N = 1,000,000 names, on a Btrfs directory:

| backend | build s | disk MiB | query p50 / p99 ms | durable update p50 / p95 ms | RSS growth after open+queries+updates |
|---|---|---|---|---|---|
| sqlite (sync=NORMAL, WAL) | 10.5 | 155.9 | 1.26 / 2.09 | 0.11 / 0.23 | 0.0 MiB |
| sqlite (sync=FULL, WAL) | 10.6 | 155.9 | 1.26 / 2.12 | **1.26 / 1.55** | 0.0 MiB |
| tantivy | 1.9 | 47.7 | 6.78 / 8.71 | 20.6 / 33.6 | 36.7 MiB |
| fst | 1.3 | 19.7 | 38.5 / 238 | 1092 / 1111 | ~0 (mmap) |
| scan | 0.1 | 20.3 | 1.44 / 26.3 | 22.2 / 25.7 | 52.0 MiB |

At N = 100,000, the SQLite FULL update p50 is 1.37 ms and Tantivy's is
22.2 ms. The query p99 values are 1.57 ms (SQLite) and 5.37 ms (Tantivy).
Every backend returned identical hit counts for every query, which serves as
a correctness cross-check.

## Decision: SQLite FTS5 (trigram)

- **Incremental updates** are 16× cheaper than Tantivy at equal durability
  (fsync per commit). The real engine batches events, so the per-event cost
  will be lower still.
- **Idle and working memory** stays flat. SQLite pages go through a bounded
  page cache (`cache_size`). Tantivy's writer needs at least 15 MB per thread
  (`MEMORY_BUDGET_NUM_BYTES_MIN`), and the reader grew 30 MiB on queries.
- **One transactional store** holds both the tree/metadata and the search
  index, so there is no dual-write consistency problem between a metadata DB
  and a search index. WAL also gives crash safety without extra code.
- **Query latency** is the best of the indexed options at 1M, at ~2 ms p99.
- **Cost:** about 3× the disk of Tantivy (156 vs 48 MiB at 1M names). That is
  acceptable, and the real schema stores less per row than this prototype's
  duplicate `key` column, because it uses parent pointers rather than full
  paths.
- FST is excellent for prefix lookups but unsuitable for substring search and
  for incremental updates without building an LSM layer. A linear scan holds
  every name in RAM, which is the opposite of the low-idle-memory goal.

**Unverified / not measured here:** Tantivy with a long-lived writer (no
reopen), Tantivy without fsync, and SQLite with a different page size. None
of these would change the conclusion on memory or update cost.
