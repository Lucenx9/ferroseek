Ordinary word queries D-Bus: p50 3.10 / p95 3.69 / p99 4.02 / max 4.19 ms
Broad query "corpus/" D-Bus: p50 3.44 / p95 3.48 / p99 3.49 / max 3.49 ms
Broad query "corpus/ report" D-Bus: p50 7.82 / p95 7.95 / p99 7.98 / max 7.98 ms
Broad query "home/" D-Bus: p50 3.40 / p95 3.42 / p99 3.44 / max 3.44 ms

#### Review query cases: five rounds × three requests, warm cache

| case / transport | median of round medians (range) ms | sample maximum ms | library CPU / wall | verified requests |
|---|---:|---:|---:|---:|
| exact-name / library | 1.455 (1.158–1.581) | 1.933 | 0.99 | 15/15 |
| exact-name / D-Bus | 1.276 (1.273–1.502) | 1.544 | — | 15/15 |
| exact-name D-Bus sample percentiles | p50 1.28 / p95 1.50 / p99 1.54 / max 1.54 ms | — | — | — |
| path-only / library | 0.715 (0.711–0.823) | 0.940 | 0.99 | 15/15 |
| path-only / D-Bus | 0.812 (0.795–0.961) | 1.054 | — | 15/15 |
| path-only D-Bus sample percentiles | p50 0.81 / p95 0.96 / p99 1.05 / max 1.05 ms | — | — | — |
| path+name / library | 0.705 (0.698–0.848) | 0.898 | 0.99 | 15/15 |
| path+name / D-Bus | 0.826 (0.808–1.028) | 1.047 | — | 15/15 |
| path+name D-Bus sample percentiles | p50 0.83 / p95 1.03 / p99 1.05 / max 1.05 ms | — | — | — |
| within+name / library | 0.053 (0.051–0.057) | 0.067 | 1.01 | 15/15 |
| path-miss / library | 0.365 (0.329–0.391) | 0.439 | 1.00 | 15/15 |
| path-miss / D-Bus | 0.488 (0.429–0.535) | 0.547 | — | 15/15 |
| path-miss D-Bus sample percentiles | p50 0.51 / p95 0.53 / p99 0.55 / max 0.55 ms | — | — | — |
| slash-only / library | 0.001 (0.001–0.001) | 0.001 | 2.12 | 15/15 |
| slash-only / D-Bus | 0.071 (0.070–0.073) | 0.075 | — | 15/15 |
| slash-only D-Bus sample percentiles | p50 0.07 / p95 0.07 / p99 0.07 / max 0.07 ms | — | — | — |

Review query errors/mismatches: 0 (165 measured responses checked; warmups also checked).

| metric | value |
|---|---|

