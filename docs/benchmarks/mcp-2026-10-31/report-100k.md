| metric | value |
|---|---|
| startup: spawn → `initialize` reply | 1.3 ms |
| `search_files` (limit 20, 330 queries, 4667 results) | p50 0.90 / p95 3.90 / p99 4.34 / max 8.21 ms |
| `search_files` with `root` filter (100) | p50 3.62 / p95 12.60 / p99 13.23 / max 14.09 ms |
| `get_file_info` (200) | p50 0.08 / p95 0.12 / p99 0.14 / max 0.14 ms |
| `index_status` (20) | p50 13.30 / p95 13.79 / p99 15.42 / max 15.42 ms |
| `search_files` response size (JSON-RPC line) | p50 6.2 KiB / max 10.6 KiB |
| RSS after initialize (anon) | 5.1 MiB (0.4 MiB), 2 threads |
| RSS after workload (anon), VmHWM | 16.2 MiB (9.7 MiB), 16.2 MiB |
| idle RSS 12 s later (anon) | 11.5 MiB (4.9 MiB), 2 threads |
| idle CPU / context switches over 30 s | 0.000 % / 0.00 per s |
