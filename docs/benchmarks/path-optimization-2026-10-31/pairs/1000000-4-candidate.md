Ordinary word queries D-Bus: p50 3.46 / p95 3.86 / p99 4.47 / max 4.79 ms
Broad query "corpus/" D-Bus: p50 3.47 / p95 3.65 / p99 3.66 / max 3.66 ms
Broad query "corpus/ report" D-Bus: p50 9.15 / p95 9.70 / p99 9.75 / max 9.75 ms
Broad query "home/" D-Bus: p50 3.39 / p95 3.63 / p99 4.05 / max 4.05 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 4.331 (4.319–4.667) | 5.064 | 1.00 | 15/15 |
| exact-name / D-Bus | 4.504 (4.496–4.558) | 5.365 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 4.51 / p95 4.67 / p99 5.37 / max 5.37 ms | — | — | — |
| path-only / library | 2.175 (1.931–2.266) | 2.504 | 0.99 | 15/15 |
| path-only / D-Bus | 2.008 (1.998–2.088) | 2.330 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 2.01 / p95 2.09 / p99 2.33 / max 2.33 ms | — | — | — |
| path+name / library | 1.920 (1.899–2.258) | 2.463 | 0.99 | 15/15 |
| path+name / D-Bus | 2.012 (1.992–2.125) | 2.342 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 2.01 / p95 2.13 / p99 2.34 / max 2.34 ms | — | — | — |
| within+name / library | 0.079 (0.074–0.085) | 0.090 | 1.01 | 15/15 |
| path-miss / library | 0.457 (0.438–0.582) | 0.595 | 0.99 | 15/15 |
| path-miss / D-Bus | 0.557 (0.490–0.596) | 0.652 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.56 / p95 0.61 / p99 0.65 / max 0.65 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.14 | 15/15 |
| slash-only / D-Bus | 0.078 (0.075–0.082) | 0.086 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.08 / p95 0.08 / p99 0.09 / max 0.09 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

