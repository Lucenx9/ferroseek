| metric | value |
|---|---|
| startup: spawn → `initialize` reply | 1.1 ms |
| `search_files` (limit 20, 330 queries, 5309 results) | p50 2.89 / p95 5.73 / p99 10.11 / max 67.86 ms |
| `search_files` with `root` filter (100) | p50 8.54 / p95 32.71 / p99 38.74 / max 40.47 ms |
| `get_file_info` (200) | p50 0.08 / p95 0.12 / p99 0.16 / max 0.17 ms |
| `index_status` (20) | p50 135.33 / p95 138.35 / p99 140.13 / max 140.13 ms |
| `search_files` response size (JSON-RPC line) | p50 6.2 KiB / max 8.7 KiB |
| RSS after initialize (anon) | 5.0 MiB (0.4 MiB), 2 threads |
| RSS after workload (anon), VmHWM | 17.5 MiB (10.9 MiB), 17.5 MiB |
| idle RSS 12 s later (anon) | 12.0 MiB (5.3 MiB), 2 threads |
| idle CPU / context switches over 30 s | 0.000 % / 0.00 per s |
