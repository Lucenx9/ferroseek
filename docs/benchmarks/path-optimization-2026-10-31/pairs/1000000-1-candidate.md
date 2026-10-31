Ordinary word queries D-Bus: p50 3.49 / p95 3.95 / p99 4.36 / max 4.55 ms
Broad query "corpus/" D-Bus: p50 3.46 / p95 3.68 / p99 4.14 / max 4.14 ms
Broad query "corpus/ report" D-Bus: p50 10.13 / p95 12.18 / p99 13.78 / max 13.78 ms
Broad query "home/" D-Bus: p50 3.41 / p95 3.63 / p99 4.10 / max 4.10 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 4.461 (4.370–4.682) | 4.824 | 0.99 | 15/15 |
| exact-name / D-Bus | 4.564 (4.510–4.708) | 5.156 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 4.56 / p95 4.71 / p99 5.16 / max 5.16 ms | — | — | — |
| path-only / library | 1.954 (1.941–2.105) | 2.310 | 0.99 | 15/15 |
| path-only / D-Bus | 2.032 (2.026–2.378) | 2.700 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 2.05 / p95 2.38 / p99 2.70 / max 2.70 ms | — | — | — |
| path+name / library | 1.962 (1.943–2.117) | 2.153 | 0.99 | 15/15 |
| path+name / D-Bus | 2.126 (2.028–2.141) | 2.521 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 2.04 / p95 2.49 / p99 2.52 / max 2.52 ms | — | — | — |
| within+name / library | 0.079 (0.076–0.089) | 0.090 | 1.01 | 15/15 |
| path-miss / library | 0.443 (0.415–0.493) | 0.530 | 1.00 | 15/15 |
| path-miss / D-Bus | 0.589 (0.520–0.628) | 0.692 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.59 / p95 0.63 / p99 0.69 / max 0.69 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.12 | 15/15 |
| slash-only / D-Bus | 0.076 (0.075–0.080) | 0.115 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.08 / p95 0.08 / p99 0.11 / max 0.11 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

