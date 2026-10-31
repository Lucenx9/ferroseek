Ordinary word queries D-Bus: p50 0.75 / p95 0.93 / p99 1.02 / max 1.10 ms
Broad query "corpus/" D-Bus: p50 3.54 / p95 4.07 / p99 4.13 / max 4.13 ms
Broad query "corpus/ report" D-Bus: p50 1.44 / p95 1.74 / p99 1.80 / max 1.80 ms
Broad query "home/" D-Bus: p50 3.44 / p95 4.07 / p99 4.10 / max 4.10 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 0.471 (0.447–0.576) | 0.636 | 0.99 | 15/15 |
| exact-name / D-Bus | 0.576 (0.501–0.586) | 0.619 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 0.58 / p95 0.62 / p99 0.62 / max 0.62 ms | — | — | — |
| path-only / library | 0.392 (0.385–0.403) | 0.461 | 0.99 | 15/15 |
| path-only / D-Bus | 4.886 (4.865–5.082) | 5.927 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 4.89 / p95 5.74 / p99 5.93 / max 5.93 ms | — | — | — |
| path+name / library | 0.434 (0.381–0.486) | 0.496 | 1.00 | 15/15 |
| path+name / D-Bus | 0.661 (0.638–0.754) | 0.772 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.66 / p95 0.75 / p99 0.77 / max 0.77 ms | — | — | — |
| within+name / library | 0.076 (0.072–0.080) | 0.091 | 1.01 | 15/15 |
| path-miss / library | 0.282 (0.257–0.292) | 0.296 | 1.00 | 15/15 |
| path-miss / D-Bus | 4.848 (4.830–5.656) | 6.074 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 4.85 / p95 5.66 / p99 6.07 / max 6.07 ms | — | — | — |
| slash-only / library | 0.001 (0.000–0.001) | 0.001 | 2.13 | 15/15 |
| slash-only / D-Bus | 0.070 (0.069–0.074) | 0.075 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.07 / p95 0.07 / p99 0.08 / max 0.08 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

