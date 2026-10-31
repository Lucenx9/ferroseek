| metric | value |
|---|---|
| startup: spawn → `initialize` reply | 1.1 ms |
| `search_files` (limit 20, 330 queries, 5309 results) | p50 4.81 / p95 12.08 / p99 15.74 / max 71.67 ms |
| `search_files` with `root` filter (100) | p50 9.12 / p95 36.22 / p99 42.34 / max 42.43 ms |
| `get_file_info` (200) | p50 0.09 / p95 0.13 / p99 0.17 / max 0.17 ms |
| `index_status` (20) | p50 0.06 / p95 0.07 / p99 0.07 / max 0.07 ms |
| `search_files` response size (JSON-RPC line) | p50 6.2 KiB / max 8.9 KiB |
| RSS after initialize (anon) | 5.1 MiB (0.4 MiB), 2 threads |
| RSS after workload (anon), VmHWM | 15.3 MiB (8.7 MiB), 15.9 MiB |
| idle RSS 12 s later (anon) | 11.4 MiB (4.8 MiB), 2 threads |
| idle CPU / context switches over 30 s | 0.000 % / 0.00 per s |
