| metric | value |
|---|---|
| startup: spawn → `initialize` reply | 1.2 ms |
| `search_files` (limit 20, 330 queries, 4667 results) | p50 1.30 / p95 9.26 / p99 9.97 / max 10.92 ms |
| `search_files` with `root` filter (100) | p50 3.92 / p95 15.96 / p99 16.83 / max 16.87 ms |
| `get_file_info` (200) | p50 0.09 / p95 0.13 / p99 0.16 / max 0.18 ms |
| `index_status` (20) | p50 0.07 / p95 0.08 / p99 0.11 / max 0.11 ms |
| `search_files` response size (JSON-RPC line) | p50 6.2 KiB / max 10.6 KiB |
| RSS after initialize (anon) | 5.0 MiB (0.4 MiB), 2 threads |
| RSS after workload (anon), VmHWM | 12.9 MiB (6.3 MiB), 13.2 MiB |
| idle RSS 12 s later (anon) | 10.4 MiB (3.8 MiB), 2 threads |
| idle CPU / context switches over 30 s | 0.000 % / 0.00 per s |
