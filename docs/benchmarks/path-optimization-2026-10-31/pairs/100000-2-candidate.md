Ordinary word queries D-Bus: p50 3.04 / p95 3.59 / p99 3.95 / max 4.05 ms
Broad query "corpus/" D-Bus: p50 3.49 / p95 3.72 / p99 4.13 / max 4.13 ms
Broad query "corpus/ report" D-Bus: p50 7.92 / p95 8.58 / p99 9.29 / max 9.29 ms
Broad query "home/" D-Bus: p50 3.42 / p95 3.44 / p99 3.46 / max 3.46 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 1.375 (1.269–1.737) | 1.761 | 0.99 | 15/15 |
| exact-name / D-Bus | 1.469 (1.350–1.622) | 1.714 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 1.40 / p95 1.62 / p99 1.71 / max 1.71 ms | — | — | — |
| path-only / library | 0.935 (0.803–1.020) | 1.111 | 1.00 | 15/15 |
| path-only / D-Bus | 1.040 (0.910–1.074) | 1.196 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 1.02 / p95 1.11 / p99 1.20 / max 1.20 ms | — | — | — |
| path+name / library | 0.806 (0.802–0.951) | 0.957 | 1.00 | 15/15 |
| path+name / D-Bus | 0.900 (0.893–1.032) | 1.076 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.91 / p95 1.04 / p99 1.08 / max 1.08 ms | — | — | — |
| within+name / library | 0.076 (0.073–0.084) | 0.091 | 1.01 | 15/15 |
| path-miss / library | 0.471 (0.453–0.544) | 0.560 | 1.00 | 15/15 |
| path-miss / D-Bus | 0.544 (0.484–0.673) | 0.736 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.55 / p95 0.67 / p99 0.74 / max 0.74 ms | — | — | — |
| slash-only / library | 0.001 (0.000–0.001) | 0.001 | 2.14 | 15/15 |
| slash-only / D-Bus | 0.066 (0.065–0.067) | 0.070 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.07 / p95 0.07 / p99 0.07 / max 0.07 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

