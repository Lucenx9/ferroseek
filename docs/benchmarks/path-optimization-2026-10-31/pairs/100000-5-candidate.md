Ordinary word queries D-Bus: p50 3.18 / p95 3.72 / p99 4.01 / max 4.44 ms
Broad query "corpus/" D-Bus: p50 3.46 / p95 3.47 / p99 3.52 / max 3.52 ms
Broad query "corpus/ report" D-Bus: p50 7.95 / p95 8.11 / p99 8.72 / max 8.72 ms
Broad query "home/" D-Bus: p50 3.41 / p95 3.66 / p99 4.51 / max 4.51 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 1.268 (1.256–1.814) | 1.871 | 1.00 | 15/15 |
| exact-name / D-Bus | 1.658 (1.424–1.798) | 2.112 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 1.67 / p95 1.86 / p99 2.11 / max 2.11 ms | — | — | — |
| path-only / library | 0.911 (0.811–1.685) | 1.837 | 0.99 | 15/15 |
| path-only / D-Bus | 0.957 (0.898–1.148) | 1.294 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 0.96 / p95 1.29 / p99 1.29 / max 1.29 ms | — | — | — |
| path+name / library | 0.831 (0.819–1.233) | 1.284 | 0.99 | 15/15 |
| path+name / D-Bus | 0.981 (0.906–1.231) | 1.288 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.95 / p95 1.23 / p99 1.29 / max 1.29 ms | — | — | — |
| within+name / library | 0.080 (0.075–0.097) | 0.138 | 1.01 | 15/15 |
| path-miss / library | 0.406 (0.375–0.746) | 0.761 | 1.00 | 15/15 |
| path-miss / D-Bus | 0.542 (0.497–0.623) | 1.008 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.54 / p95 0.84 / p99 1.01 / max 1.01 ms | — | — | — |
| slash-only / library | 0.000 (0.000–0.001) | 0.001 | 2.12 | 15/15 |
| slash-only / D-Bus | 0.073 (0.066–0.100) | 0.109 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.09 / p95 0.10 / p99 0.11 / max 0.11 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

