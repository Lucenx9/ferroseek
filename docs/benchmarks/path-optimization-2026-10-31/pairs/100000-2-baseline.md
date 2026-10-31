Ordinary word queries D-Bus: p50 3.27 / p95 3.79 / p99 4.17 / max 4.99 ms
Broad query "corpus/" D-Bus: p50 3.47 / p95 3.48 / p99 3.49 / max 3.49 ms
Broad query "corpus/ report" D-Bus: p50 10.91 / p95 11.06 / p99 11.67 / max 11.67 ms
Broad query "home/" D-Bus: p50 3.47 / p95 3.49 / p99 3.52 / max 3.52 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 1.494 (1.237–1.622) | 1.715 | 0.99 | 15/15 |
| exact-name / D-Bus | 1.356 (1.350–1.547) | 1.576 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 1.36 / p95 1.55 / p99 1.58 / max 1.58 ms | — | — | — |
| path-only / library | 0.858 (0.787–0.987) | 0.999 | 0.99 | 15/15 |
| path-only / D-Bus | 52.315 (51.310–52.733) | 53.702 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 51.81 / p95 52.94 / p99 53.70 / max 53.70 ms | — | — | — |
| path+name / library | 0.821 (0.791–0.977) | 1.068 | 0.99 | 15/15 |
| path+name / D-Bus | 8.353 (8.045–8.535) | 9.519 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 8.25 / p95 9.19 / p99 9.52 / max 9.52 ms | — | — | — |
| within+name / library | 0.082 (0.074–0.089) | 0.095 | 1.01 | 15/15 |
| path-miss / library | 0.404 (0.382–0.452) | 0.472 | 1.00 | 15/15 |
| path-miss / D-Bus | 51.335 (51.239–51.893) | 53.276 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 51.58 / p95 52.52 / p99 53.28 / max 53.28 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.12 | 15/15 |
| slash-only / D-Bus | 0.068 (0.067–0.071) | 0.076 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.07 / p95 0.07 / p99 0.08 / max 0.08 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

