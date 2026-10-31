Ordinary word queries D-Bus: p50 3.24 / p95 3.67 / p99 4.13 / max 4.64 ms
Broad query "corpus/" D-Bus: p50 3.49 / p95 3.50 / p99 3.56 / max 3.56 ms
Broad query "corpus/ report" D-Bus: p50 10.90 / p95 10.92 / p99 10.94 / max 10.94 ms
Broad query "home/" D-Bus: p50 3.50 / p95 3.52 / p99 3.54 / max 3.54 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 1.070 (1.069–1.247) | 1.471 | 1.00 | 15/15 |
| exact-name / D-Bus | 1.201 (1.180–1.433) | 1.735 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 1.20 / p95 1.43 / p99 1.74 / max 1.74 ms | — | — | — |
| path-only / library | 0.654 (0.645–0.779) | 0.850 | 1.00 | 15/15 |
| path-only / D-Bus | 52.359 (51.391–53.561) | 55.203 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 52.36 / p95 53.56 / p99 55.20 / max 55.20 ms | — | — | — |
| path+name / library | 0.748 (0.650–0.793) | 0.857 | 0.99 | 15/15 |
| path+name / D-Bus | 8.086 (8.014–8.497) | 9.243 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 8.09 / p95 9.22 / p99 9.24 / max 9.24 ms | — | — | — |
| within+name / library | 0.052 (0.050–0.059) | 0.068 | 1.01 | 15/15 |
| path-miss / library | 0.296 (0.275–0.317) | 0.352 | 1.00 | 15/15 |
| path-miss / D-Bus | 51.902 (51.174–53.452) | 53.630 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 51.91 / p95 53.45 / p99 53.63 / max 53.63 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.10 | 15/15 |
| slash-only / D-Bus | 0.074 (0.073–0.081) | 0.085 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.07 / p95 0.08 / p99 0.08 / max 0.08 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

