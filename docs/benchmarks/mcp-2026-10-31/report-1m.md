| metric | value |
|---|---|
| startup: spawn → `initialize` reply | 1.2 ms |
| `search_files` (limit 20, 330 queries, 5309 results) | p50 3.22 / p95 6.18 / p99 11.01 / max 65.64 ms |
| `search_files` with `root` filter (100) | p50 10.75 / p95 34.26 / p99 38.56 / max 39.12 ms |
| `get_file_info` (200) | p50 0.09 / p95 0.15 / p99 0.18 / max 0.21 ms |
| `index_status` (20) | p50 0.06 / p95 0.07 / p99 0.08 / max 0.08 ms |
| `search_files` response size (JSON-RPC line) | p50 6.2 KiB / max 8.7 KiB |
| RSS after initialize (anon) | 5.1 MiB (0.4 MiB), 2 threads |
| RSS after workload (anon), VmHWM | 15.5 MiB (9.0 MiB), 15.5 MiB |
| idle RSS 12 s later (anon) | 10.6 MiB (4.0 MiB), 2 threads |
| idle CPU / context switches over 30 s | 0.000 % / 0.00 per s |
