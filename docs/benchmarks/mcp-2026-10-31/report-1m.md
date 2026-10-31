| metric | value |
|---|---|
| startup: spawn → `initialize` reply | 1.2 ms |
| `search_files` (limit 20, 330 queries, 5309 results) | p50 5.69 / p95 13.91 / p99 18.87 / max 70.85 ms |
| `search_files` with `root` filter (100) | p50 9.91 / p95 35.80 / p99 40.84 / max 41.14 ms |
| `get_file_info` (200) | p50 0.09 / p95 0.13 / p99 0.14 / max 0.19 ms |
| `index_status` (20) | p50 0.07 / p95 0.09 / p99 0.10 / max 0.10 ms |
| `search_files` response size (JSON-RPC line) | p50 6.2 KiB / max 8.6 KiB |
| RSS after initialize (anon) | 5.1 MiB (0.4 MiB), 2 threads |
| RSS after workload (anon), VmHWM | 14.3 MiB (7.7 MiB), 15.3 MiB |
| idle RSS 12 s later (anon) | 10.3 MiB (3.6 MiB), 2 threads |
| idle CPU / context switches over 30 s | 0.000 % / 0.00 per s |
