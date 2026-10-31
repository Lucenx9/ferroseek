Ordinary word queries D-Bus: p50 3.53 / p95 4.05 / p99 4.39 / max 4.64 ms
Broad query "corpus/" D-Bus: p50 3.46 / p95 3.81 / p99 4.56 / max 4.56 ms
Broad query "corpus/ report" D-Bus: p50 9.42 / p95 10.28 / p99 10.40 / max 10.40 ms
Broad query "home/" D-Bus: p50 3.40 / p95 3.69 / p99 4.17 / max 4.17 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 4.394 (4.387–4.799) | 5.221 | 1.00 | 15/15 |
| exact-name / D-Bus | 4.420 (4.361–4.893) | 5.476 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 4.42 / p95 4.89 / p99 5.48 / max 5.48 ms | — | — | — |
| path-only / library | 1.850 (1.846–1.983) | 2.166 | 1.00 | 15/15 |
| path-only / D-Bus | 1.913 (1.903–2.042) | 2.227 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 1.92 / p95 2.04 / p99 2.23 / max 2.23 ms | — | — | — |
| path+name / library | 1.846 (1.839–2.104) | 2.222 | 1.00 | 15/15 |
| path+name / D-Bus | 1.934 (1.920–2.039) | 2.198 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 1.93 / p95 2.04 / p99 2.20 / max 2.20 ms | — | — | — |
| within+name / library | 0.066 (0.064–0.074) | 0.078 | 1.01 | 15/15 |
| path-miss / library | 0.338 (0.333–0.355) | 0.364 | 1.00 | 15/15 |
| path-miss / D-Bus | 0.535 (0.501–0.604) | 0.653 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.54 / p95 0.60 / p99 0.65 / max 0.65 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.07 | 15/15 |
| slash-only / D-Bus | 0.084 (0.083–0.086) | 0.091 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.08 / p95 0.09 / p99 0.09 / max 0.09 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

