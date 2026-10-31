| metric | value |
|---|---|
| startup: spawn → `initialize` reply | 1.2 ms |
| `search_files` (limit 20, 330 queries, 4667 results) | p50 1.31 / p95 9.02 / p99 10.04 / max 10.78 ms |
| `search_files` with `root` filter (100) | p50 4.09 / p95 15.50 / p99 16.02 / max 16.23 ms |
| `get_file_info` (200) | p50 0.09 / p95 0.13 / p99 0.14 / max 0.15 ms |
| `index_status` (20) | p50 0.06 / p95 0.07 / p99 0.07 / max 0.07 ms |
| `search_files` response size (JSON-RPC line) | p50 6.2 KiB / max 10.6 KiB |
| RSS after initialize (anon) | 5.0 MiB (0.4 MiB), 2 threads |
| RSS after workload (anon), VmHWM | 13.5 MiB (6.9 MiB), 13.5 MiB |
| idle RSS 12 s later (anon) | 9.8 MiB (3.2 MiB), 2 threads |
| idle CPU / context switches over 30 s | 0.000 % / 0.00 per s |
