Ordinary word queries D-Bus: p50 0.62 / p95 0.75 / p99 0.87 / max 0.91 ms
Broad query "corpus/" D-Bus: p50 3.34 / p95 3.35 / p99 3.48 / max 3.48 ms
Broad query "corpus/ report" D-Bus: p50 1.12 / p95 1.14 / p99 1.15 / max 1.15 ms
Broad query "home/" D-Bus: p50 3.30 / p95 3.33 / p99 3.33 / max 3.33 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 0.610 (0.538–0.645) | 0.730 | 1.00 | 15/15 |
| exact-name / D-Bus | 0.651 (0.599–0.672) | 0.695 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 0.65 / p95 0.68 / p99 0.70 / max 0.70 ms | — | — | — |
| path-only / library | 0.528 (0.477–0.590) | 0.611 | 1.00 | 15/15 |
| path-only / D-Bus | 0.621 (0.536–0.636) | 0.653 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 0.62 / p95 0.64 / p99 0.65 / max 0.65 ms | — | — | — |
| path+name / library | 0.509 (0.451–0.579) | 0.583 | 0.99 | 15/15 |
| path+name / D-Bus | 0.632 (0.530–0.700) | 0.734 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.62 / p95 0.72 / p99 0.73 / max 0.73 ms | — | — | — |
| within+name / library | 0.075 (0.074–0.082) | 0.093 | 1.01 | 15/15 |
| path-miss / library | 0.352 (0.323–0.373) | 0.432 | 0.99 | 15/15 |
| path-miss / D-Bus | 0.461 (0.432–0.480) | 0.500 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.45 / p95 0.49 / p99 0.50 / max 0.50 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.12 | 15/15 |
| slash-only / D-Bus | 0.078 (0.078–0.086) | 0.087 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.08 / p95 0.09 / p99 0.09 / max 0.09 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

