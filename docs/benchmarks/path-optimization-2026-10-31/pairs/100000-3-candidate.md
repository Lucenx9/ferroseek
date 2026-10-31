Ordinary word queries D-Bus: p50 3.14 / p95 3.46 / p99 3.84 / max 4.14 ms
Broad query "corpus/" D-Bus: p50 3.62 / p95 4.24 / p99 4.29 / max 4.29 ms
Broad query "corpus/ report" D-Bus: p50 7.96 / p95 8.40 / p99 9.14 / max 9.14 ms
Broad query "home/" D-Bus: p50 3.41 / p95 3.68 / p99 4.19 / max 4.19 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 1.102 (1.102–1.559) | 1.858 | 1.00 | 15/15 |
| exact-name / D-Bus | 1.237 (1.233–1.271) | 1.277 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 1.24 / p95 1.27 / p99 1.28 / max 1.28 ms | — | — | — |
| path-only / library | 0.654 (0.638–0.821) | 0.846 | 1.00 | 15/15 |
| path-only / D-Bus | 0.774 (0.751–0.904) | 0.910 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 0.77 / p95 0.90 / p99 0.91 / max 0.91 ms | — | — | — |
| path+name / library | 0.645 (0.635–0.771) | 0.831 | 1.00 | 15/15 |
| path+name / D-Bus | 0.768 (0.744–0.943) | 0.957 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.77 / p95 0.95 / p99 0.96 / max 0.96 ms | — | — | — |
| within+name / library | 0.052 (0.050–0.075) | 0.091 | 0.99 | 15/15 |
| path-miss / library | 0.321 (0.286–0.344) | 0.380 | 0.98 | 15/15 |
| path-miss / D-Bus | 0.415 (0.401–0.454) | 0.480 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.41 / p95 0.45 / p99 0.48 / max 0.48 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.12 | 15/15 |
| slash-only / D-Bus | 0.073 (0.073–0.080) | 0.102 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.07 / p95 0.08 / p99 0.10 / max 0.10 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

