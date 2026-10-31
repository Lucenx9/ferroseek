Ordinary word queries D-Bus: p50 3.23 / p95 3.75 / p99 4.10 / max 4.32 ms
Broad query "corpus/" D-Bus: p50 3.56 / p95 4.10 / p99 4.31 / max 4.31 ms
Broad query "corpus/ report" D-Bus: p50 10.76 / p95 11.28 / p99 11.55 / max 11.55 ms
Broad query "home/" D-Bus: p50 3.48 / p95 4.14 / p99 4.15 / max 4.15 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 1.258 (1.164–1.397) | 1.809 | 1.00 | 15/15 |
| exact-name / D-Bus | 1.308 (1.293–1.317) | 1.335 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 1.31 / p95 1.32 / p99 1.34 / max 1.34 ms | — | — | — |
| path-only / library | 0.702 (0.698–0.824) | 0.852 | 1.00 | 15/15 |
| path-only / D-Bus | 51.519 (51.243–52.433) | 52.963 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 51.52 / p95 52.94 / p99 52.96 / max 52.96 ms | — | — | — |
| path+name / library | 0.700 (0.697–0.713) | 0.730 | 1.00 | 15/15 |
| path+name / D-Bus | 8.048 (8.026–8.365) | 8.456 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 8.07 / p95 8.36 / p99 8.46 / max 8.46 ms | — | — | — |
| within+name / library | 0.053 (0.051–0.077) | 0.121 | 0.98 | 15/15 |
| path-miss / library | 0.376 (0.335–0.402) | 0.459 | 0.99 | 15/15 |
| path-miss / D-Bus | 52.096 (51.372–53.014) | 54.183 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 52.17 / p95 53.95 / p99 54.18 / max 54.18 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.08 | 15/15 |
| slash-only / D-Bus | 0.066 (0.064–0.072) | 0.083 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.07 / p95 0.08 / p99 0.08 / max 0.08 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

