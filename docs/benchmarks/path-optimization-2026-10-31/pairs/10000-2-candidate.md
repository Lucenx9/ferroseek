Ordinary word queries D-Bus: p50 0.60 / p95 0.77 / p99 0.89 / max 0.93 ms
Broad query "corpus/" D-Bus: p50 3.33 / p95 3.56 / p99 4.05 / max 4.05 ms
Broad query "corpus/ report" D-Bus: p50 1.09 / p95 1.16 / p99 1.30 / max 1.30 ms
Broad query "home/" D-Bus: p50 3.33 / p95 3.81 / p99 3.83 / max 3.83 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 0.441 (0.422–0.593) | 0.638 | 0.99 | 15/15 |
| exact-name / D-Bus | 0.553 (0.542–0.645) | 0.794 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 0.56 / p95 0.68 / p99 0.79 / max 0.79 ms | — | — | — |
| path-only / library | 0.417 (0.370–0.480) | 0.539 | 0.99 | 15/15 |
| path-only / D-Bus | 0.487 (0.442–0.574) | 0.584 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 0.49 / p95 0.57 / p99 0.58 / max 0.58 ms | — | — | — |
| path+name / library | 0.412 (0.375–0.450) | 0.464 | 1.00 | 15/15 |
| path+name / D-Bus | 0.529 (0.503–0.655) | 0.702 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.54 / p95 0.66 / p99 0.70 / max 0.70 ms | — | — | — |
| within+name / library | 0.075 (0.069–0.094) | 0.124 | 0.99 | 15/15 |
| path-miss / library | 0.247 (0.225–0.265) | 0.273 | 1.00 | 15/15 |
| path-miss / D-Bus | 0.360 (0.358–0.394) | 0.436 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.37 / p95 0.39 / p99 0.44 / max 0.44 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.11 | 15/15 |
| slash-only / D-Bus | 0.075 (0.069–0.133) | 0.151 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.08 / p95 0.13 / p99 0.15 / max 0.15 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

