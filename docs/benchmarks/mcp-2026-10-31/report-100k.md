| metric | value |
|---|---|
| startup: spawn → `initialize` reply | 1.1 ms |
| `search_files` (limit 20, 330 queries, 4667 results) | p50 1.08 / p95 4.12 / p99 4.67 / max 9.47 ms |
| `search_files` with `root` filter (100) | p50 3.92 / p95 12.58 / p99 13.04 / max 14.14 ms |
| `get_file_info` (200) | p50 0.10 / p95 0.13 / p99 0.17 / max 0.18 ms |
| `index_status` (20) | p50 0.07 / p95 0.09 / p99 0.09 / max 0.09 ms |
| `search_files` response size (JSON-RPC line) | p50 6.2 KiB / max 10.6 KiB |
| RSS after initialize (anon) | 5.2 MiB (0.4 MiB), 2 threads |
| RSS after workload (anon), VmHWM | 13.3 MiB (6.7 MiB), 13.4 MiB |
| idle RSS 12 s later (anon) | 10.1 MiB (3.5 MiB), 2 threads |
| idle CPU / context switches over 30 s | 0.000 % / 0.00 per s |
