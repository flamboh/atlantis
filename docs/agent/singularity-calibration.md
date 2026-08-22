# Rust Singularity conformance and threshold calibration

## Summary

I compared the Rust port with the prebuilt Haskell reference on 24 real five-minute windows and calibrated the Rust alert feed on 200 separate windows. The conformance verdict is PASS. Every compared address and `n_levels` value matched, and no numeric value violated the requested tolerance.

The calibration supports `threshold_high = 2` and `threshold_low = 0.3`. At the feed's 20-per-tail cap, this pair produces a mean of 19.7 and median of 20.0 recorded alerts per window. The middle 80% spans 14.0 to 25.0.

## Environment and extraction contract

The Rust binary was `netflow-db 0.1.0`, built from commit `a30a51f41793ec88c99e07bf4bb2aa8078345192` with the repository's Rust 1.97.1 toolchain. Its SHA-256 was `a3258b36c29021a6056cdbb53633313d3200884ce8a36e61b62f8ec6e9f1cac6`. The prebuilt Haskell binary SHA-256 was `ea5b0ccca94355cafbebe6f3c2a9ca3e8441b2cb11406a93b8f67e4971f81d69`; I did not rebuild it. Capture reads used nfdump 1.7.6-release on Linux 6.18.35 x86_64.

For each timestamp, the extractor reads both `cc_ir1_gw` and `oh_ir1_gw` files with `nfdump -q -o 'csv:%sa,%da'`, rejects fields containing `:`, combines source and destination addresses, and runs one locale-fixed external `sort -u`. The standard CSV probe reported `srcAddr` and `dstAddr` as columns 4 and 6. A 10,000-record validation found 5,149 unique IPv4 addresses in both the standard ten-column output and the custom two-column projection, with byte-identical sorted lists.

The capture inventory contained 111,272 paired timestamps from 2025-06-01 00:00 through 2026-06-30 13:15. The selector excluded one cc-only and two oh-only timestamps. Filename hours are treated as America/Los_Angeles local time, matching the feed's configured timezone.

## Part 1: Rust versus Haskell conformance

### Method

The deterministic selector chose 24 windows across all 13 available months. I kept all 24 candidates, instead of stopping at 20, so May and June 2026 remained represented. It used 03:00, 09:00, 15:00, and 21:00 hours six times each, with 16 weekday and 8 weekend windows. Both implementations received the exact same sorted address file. I joined results by address, compared `n_levels` with exact integer equality, and applied `abs(a-b) <= max(1e-9 * max(abs(a),abs(b)), 1e-12)` to alpha, intercept, and r2.

The deviation columns below report `abs(a-b) / max(abs(a),abs(b),1e-12)`. Near zero, that display value can exceed 1e-9 while the absolute 1e-12 tolerance still passes. The A/I/R violations column removes that ambiguity.

| Timestamp    | Addresses | Max rel alpha | Max rel intercept | Max rel r2 | A/I/R violations | Level mismatches | Rust/Haskell only |
| ------------ | --------- | ------------- | ----------------- | ---------- | ---------------- | ---------------- | ----------------- |
| 202506080355 | 296,779   | 7.270e-15     | 1.474e-10         | 6.947e-15  | 0/0/0            | 0                | 0/0               |
| 202506250900 | 309,275   | 3.911e-15     | 4.524e-09         | 6.901e-15  | 0/0/0            | 0                | 0/0               |
| 202507111555 | 299,657   | 3.817e-15     | 5.471e-10         | 6.190e-15  | 0/0/0            | 0                | 0/0               |
| 202507272155 | 294,684   | 4.441e-15     | 6.094e-11         | 1.168e-14  | 0/0/0            | 0                | 0/0               |
| 202508140300 | 302,023   | 5.717e-15     | 1.033e-10         | 8.630e-15  | 0/0/0            | 0                | 0/0               |
| 202508290955 | 301,098   | 4.400e-15     | 1.066e-10         | 6.494e-15  | 0/0/0            | 0                | 0/0               |
| 202509141555 | 291,695   | 4.422e-15     | 2.007e-10         | 7.696e-15  | 0/0/0            | 0                | 0/0               |
| 202510022100 | 330,546   | 4.318e-15     | 1.182e-09         | 7.520e-15  | 0/0/0            | 0                | 0/0               |
| 202510170355 | 294,497   | 7.737e-15     | 2.417e-10         | 8.917e-15  | 0/0/0            | 0                | 0/0               |
| 202511020955 | 309,703   | 4.559e-15     | 7.636e-11         | 7.037e-15  | 0/0/0            | 0                | 0/0               |
| 202511171555 | 316,316   | 7.450e-15     | 1.004e-10         | 7.095e-15  | 0/0/0            | 0                | 0/0               |
| 202512082100 | 318,123   | 5.220e-15     | 7.459e-10         | 7.741e-15  | 0/0/0            | 0                | 0/0               |
| 202512210355 | 290,362   | 4.610e-15     | 2.848e-09         | 8.392e-15  | 0/0/0            | 0                | 0/0               |
| 202601090900 | 373,695   | 4.797e-15     | 1.187e-09         | 8.681e-15  | 0/0/0            | 0                | 0/0               |
| 202601261500 | 321,101   | 5.734e-15     | 1.404e-08         | 7.690e-15  | 0/0/0            | 0                | 0/0               |
| 202602082155 | 305,401   | 6.569e-15     | 1.324e-10         | 7.569e-15  | 0/0/0            | 0                | 0/0               |
| 202602270300 | 311,010   | 5.386e-15     | 1.380e-10         | 9.091e-15  | 0/0/0            | 0                | 0/0               |
| 202603160900 | 335,478   | 4.635e-15     | 1.410e-10         | 6.593e-15  | 0/0/0            | 0                | 0/0               |
| 202603291555 | 329,895   | 5.980e-15     | 5.910e-11         | 8.342e-15  | 0/0/0            | 0                | 0/0               |
| 202604172100 | 343,949   | 4.615e-15     | 1.824e-10         | 7.257e-15  | 0/0/0            | 0                | 0/0               |
| 202605040300 | 348,968   | 4.265e-15     | 2.754e-10         | 6.557e-15  | 0/0/0            | 0                | 0/0               |
| 202605170955 | 346,383   | 7.401e-15     | 1.073e-08         | 7.783e-15  | 0/0/0            | 0                | 0/0               |
| 202606051555 | 338,304   | 5.021e-15     | 1.737e-10         | 7.039e-15  | 0/0/0            | 0                | 0/0               |
| 202606222100 | 324,410   | 4.557e-15     | 1.269e-10         | 7.463e-15  | 0/0/0            | 0                | 0/0               |

Overall maxima were 7.737e-15 for alpha, 1.404e-08 for intercept, and 1.168e-14 for r2. The total mismatch or tolerance-violation count was 0. The largest tested window was 202601090900 with 373,695 addresses.

The largest displayed intercept ratio came from two near-zero values at 72.5.189.239: Rust -1.642975968608e-07, Haskell -1.642975945543e-07. Their absolute difference was 2.306e-15, versus an allowed 1.000e-12. This is the expected absolute-tolerance case, not a numerical failure.

### Haskell wall-clock budget

No window was skipped, replaced, sampled down, or timed out. The firm budget was 600 seconds per Haskell run. Observed Haskell time had a median of 11.7 seconds and a maximum of 16.0 seconds. The 299,472-address format probe took 9.1 seconds in Haskell and 3.7 seconds in Rust, so full-window checks were comfortably tractable.

## Part 2: Rust threshold calibration

### Sampling and statistics

The calibration used 200 unique paired windows from 202506010000 through 202606301100. Coverage included 13 months and all 24 hours, with 8 or 9 samples per hour, 14 to 16 per month, 56 weekend windows, and 144 weekday windows. The selector chose the nearest available pair around evenly spaced date-bin midpoints and permuted requested hours with `(index * 5) % 24`. Percentiles use Type 7 linear interpolation at rank `(n - 1) * p`.

All 200 windows completed. Rust emitted 0 non-finite alpha values across the sample.

Across-window distribution of each per-window statistic:

| Statistic   | Mean    | SD     | Min     | p10     | Median  | p90     | Max     |
| ----------- | ------- | ------ | ------- | ------- | ------- | ------- | ------- |
| n_addresses | 318,709 | 21,833 | 274,916 | 296,414 | 314,254 | 349,859 | 396,341 |
| min         | 0.199   | 0.024  | 0.170   | 0.178   | 0.191   | 0.229   | 0.277   |
| p0_1        | 0.364   | 0.010  | 0.348   | 0.353   | 0.362   | 0.379   | 0.396   |
| p1          | 0.394   | 0.009  | 0.371   | 0.383   | 0.392   | 0.408   | 0.421   |
| p5          | 0.439   | 0.009  | 0.406   | 0.425   | 0.439   | 0.449   | 0.458   |
| median      | 0.511   | 0.002  | 0.508   | 0.509   | 0.511   | 0.514   | 0.519   |
| p95         | 1.021   | 0.006  | 1.006   | 1.013   | 1.021   | 1.028   | 1.055   |
| p99         | 1.190   | 0.004  | 1.174   | 1.184   | 1.190   | 1.195   | 1.202   |
| p99_9       | 1.379   | 0.012  | 1.345   | 1.363   | 1.380   | 1.392   | 1.410   |
| max         | 3.194   | 1.155  | 2.013   | 2.149   | 2.265   | 4.507   | 4.917   |

Eight representative windows:

| Timestamp    | Addresses | Min   | p0.1  | Median | p99.9 | Max   |
| ------------ | --------- | ----- | ----- | ------ | ----- | ----- |
| 202506010000 | 298,018   | 0.180 | 0.352 | 0.510  | 1.354 | 4.457 |
| 202507272015 | 300,483   | 0.177 | 0.360 | 0.510  | 1.392 | 4.457 |
| 202509222130 | 315,015   | 0.181 | 0.359 | 0.510  | 1.377 | 4.509 |
| 202511161745 | 331,615   | 0.203 | 0.363 | 0.513  | 1.373 | 2.193 |
| 202601131800 | 341,425   | 0.231 | 0.368 | 0.514  | 1.365 | 4.539 |
| 202603091415 | 316,560   | 0.198 | 0.361 | 0.512  | 1.385 | 2.239 |
| 202605051545 | 360,662   | 0.200 | 0.371 | 0.516  | 1.363 | 2.214 |
| 202606301100 | 349,565   | 0.190 | 0.369 | 0.514  | 1.357 | 2.209 |

### Tail stability and time effects

The table compares overall spread with date, hour, weekend, and month groupings. Date uses Pearson correlation against capture time. Hour and month cells show the groups with the lowest and highest means. Weekday/weekend is shown in that order.

| Metric | Overall SD | Overall p10 to p90 | Date r | Hourly mean low/high    | Weekday/weekend mean | Monthly mean low/high        |
| ------ | ---------- | ------------------ | ------ | ----------------------- | -------------------- | ---------------------------- |
| min    | 0.024      | 0.178 to 0.229     | 0.052  | 0:00 0.186; 2:00 0.225  | 0.201/0.193          | 2026-03 0.190; 2025-12 0.207 |
| p0_1   | 0.010      | 0.353 to 0.379     | 0.423  | 2:00 0.359; 5:00 0.370  | 0.366/0.361          | 2025-08 0.356; 2026-05 0.374 |
| p99_9  | 0.012      | 1.363 to 1.392     | -0.360 | 9:00 1.373; 21:00 1.386 | 1.377/1.383          | 2026-05 1.368; 2025-08 1.387 |
| max    | 1.155      | 2.149 to 4.507     | -0.607 | 4:00 2.724; 18:00 3.663 | 3.247/3.057          | 2025-11 2.178; 2025-06 4.509 |

The center of the alpha distribution is tight: the per-window median has mean 0.511 and SD 0.002. The p0.1 and p99.9 tails have SDs of 0.010 and 0.012. Min and max are much noisier because a single address controls each value. The table keeps network growth and calendar effects separate from that single-address churn instead of treating every max swing as a feed-wide shift.

There is a measured date shift, but it is small in absolute alpha terms. Address count rose with date at r=0.697; median alpha also rose at r=0.719, while its SD stayed 0.002. The p0.1 mean rose from a monthly low of 0.356 in August 2025 to 0.374 in May 2026, with date r=0.423. The p99.9 mean moved the other way, from 1.387 in August 2025 to 1.368 in May 2026, with r=-0.360.

Time-of-day and weekday effects were smaller. Hourly p0.1 means ranged from 0.359 to 0.370, and hourly p99.9 means ranged from 1.373 to 1.386. Weekday versus weekend means differed by 0.005 for p0.1 and 0.006 for p99.9. Max alpha was the unstable statistic: SD 1.155, p10 to p90 2.149 to 4.507, and date r=-0.607. That reflects individual recurring addresses entering or leaving the tail, not a broad distribution shift.

High alpha marks sparse, isolated address-space regions. Low alpha marks addresses that remain inside dense prefix clusters. Both tails therefore remain in the recommendation.

### Initial candidate grid

Counts here use the requested strict comparisons. Capped values apply the feed's 20-per-tail limit before combining tails.

| High | Low | Raw mean | Raw median | Capped mean | Capped median |
| ---- | --- | -------- | ---------- | ----------- | ------------- |
| 2.5  | 0.3 | 6.5      | 6.0        | 5.7         | 6.0           |
| 2.5  | 0.5 | 129,919  | 128,360    | 20.4        | 20.0          |
| 2.5  | 0.7 | 245,417  | 241,792    | 20.4        | 20.0          |
| 3.0  | 0.3 | 6.5      | 6.0        | 5.7         | 6.0           |
| 3.0  | 0.5 | 129,919  | 128,360    | 20.4        | 20.0          |
| 3.0  | 0.7 | 245,417  | 241,792    | 20.4        | 20.0          |
| 3.5  | 0.3 | 6.5      | 6.0        | 5.7         | 6.0           |
| 3.5  | 0.5 | 129,919  | 128,360    | 20.4        | 20.0          |
| 3.5  | 0.7 | 245,417  | 241,792    | 20.4        | 20.0          |

The initial high range of 2.5 to 3.5 is too conservative on this feed. It contributes almost no high-tail volume, leaving the low tail to determine the result. A high threshold of 2.0 restores a useful sparse-address signal without pinning the high tail at its cap.

### Recommendation

Use `threshold_high = 2` and `threshold_low = 0.3` as fixed global constants. Before caps, high 2.0 produces mean/median counts of 14.7/14.0 and low 0.3 produces 6.1/5.0. Combined raw mean/median is 20.8/20.0. After the 20-per-tail caps, combined mean/median is 19.7/20.0.

The saved grid uses strict `>` and `<` as requested. The Rust feed uses inclusive `>=` and `<=`. Exactly 0 sampled address scores landed on either recommended constant, so inclusive feed-visible mean/median remains 19.7/20.0.

I prefer 2.0 and 0.3 because they are round, give both tails room to contribute, and put a typical window inside the requested 5 to 30 recorded-alert range. The p10 to p90 capped range is 14 to 25. One May 2026 window had 177 raw low-tail crossings and 194 raw combined crossings; the per-tail caps reduced it to 37 recorded alerts. None of the 200 selected windows was quiet enough to produce zero at this pair, but an empty or genuinely low-traffic window can still do so.

### Repeat offenders

I selected eight evenly spaced calibration windows and retained every address beyond the recommended strict thresholds.

| Timestamp    | High | Low | Combined |
| ------------ | ---- | --- | -------- |
| 202506010000 | 5    | 4   | 9        |
| 202507272015 | 17   | 6   | 23       |
| 202509222130 | 16   | 6   | 22       |
| 202511161745 | 15   | 6   | 21       |
| 202601131800 | 18   | 4   | 22       |
| 202603091415 | 17   | 5   | 22       |
| 202605051545 | 19   | 7   | 26       |
| 202606301100 | 13   | 6   | 19       |

Pairwise overlap across all 28 window pairs:

| Tail     | Mean Jaccard | Median Jaccard | Jaccard range  | Mean intersection | Intersection range |
| -------- | ------------ | -------------- | -------------- | ----------------- | ------------------ |
| high     | 0.008        | 0.000          | 0.000 to 0.050 | 0.2               | 0 to 1             |
| low      | 0.487        | 0.444          | 0.300 to 0.833 | 3.5               | 3 to 5             |
| combined | 0.102        | 0.101          | 0.065 to 0.148 | 3.7               | 3 to 5             |

Addresses recurring in at least two of the eight windows, limited to the ten most frequent:

| Tail | Address         | Windows | Min alpha | Max alpha |
| ---- | --------------- | ------- | --------- | --------- |
| low  | 224.0.0.1       | 8/8     | 0.267     | 0.273     |
| low  | 224.0.0.13      | 8/8     | 0.289     | 0.296     |
| low  | 224.0.0.2       | 8/8     | 0.267     | 0.273     |
| low  | 72.4.181.18     | 5/8     | 0.215     | 0.231     |
| high | 239.255.255.250 | 4/8     | 4.457     | 4.539     |
| low  | 72.5.64.18      | 3/8     | 0.198     | 0.214     |
| low  | 60.247.96.22    | 2/8     | 0.190     | 0.200     |

The two tails behave differently. High-tail membership mostly churned, with mean Jaccard 0.008 and pairwise intersections of at most one address. The only recurring high address was 239.255.255.250, present in four of eight windows. The low tail had a stable core: 224.0.0.1, 224.0.0.2, and 224.0.0.13 appeared in all eight windows, and low-tail mean Jaccard was 0.487. The feed should therefore expect repeat multicast-style low alerts alongside a changing high tail.

## How to reproduce

All scripts, raw outputs, address lists, logs, and summaries are under `/tmp/singularity-calibration/`. Nothing from this run was written to tracked repository files. The Cargo build wrote only the permitted `target/` output.

Key audit files:

- `inventory/conformance_candidates.txt` and `inventory/calibration_timestamps.txt` contain the selected timestamps.
- `inventory/*_coverage.csv` records dates, hours, day types, and both capture paths.
- `data/conformance_per_window/` and `data/calibration_per_window/` contain every per-window CSV summary.
- `data/conformance_windows.csv` and `data/calibration_windows.csv` are the consolidated tables.
- `data/conformance_worst_deviations.csv` records the address and absolute difference behind each displayed maximum.
- `data/threshold_grid.csv`, `data/threshold_tail_sensitivity.csv`, and `data/threshold_pair_sensitivity.csv` contain the threshold scan.
- `data/repeat_*.csv` contains the repeat-offender sets and overlaps.
- `work/conformance/` retains Rust and Haskell CSVs plus exact address lists. `work/calibration/` retains all Rust CSVs and address lists.

Run these commands from a fresh `/tmp/singularity-calibration/` layout after restoring the scripts:

```sh
cd /home/obo/.t3/worktrees/netflow-analysis/t3code-5e5d2a1c
cargo build --release -p atlantis-netflow-db
/tmp/singularity-calibration/scripts/select_windows.py
/tmp/singularity-calibration/scripts/run_conformance.py /tmp/singularity-calibration/inventory/conformance_candidates.txt target/release/netflow-db --target 24 --workers 2 --timeout-seconds 600
/tmp/singularity-calibration/scripts/run_calibration.py /tmp/singularity-calibration/inventory/calibration_timestamps.txt target/release/netflow-db --workers 4
/tmp/singularity-calibration/scripts/find_worst_deviations.py
/tmp/singularity-calibration/scripts/augment_candidate_pairs.py /tmp/singularity-calibration/data/calibration_windows.csv /tmp/singularity-calibration/data/calibration_per_window
/tmp/singularity-calibration/scripts/scan_thresholds.py /tmp/singularity-calibration/work/calibration/scores /tmp/singularity-calibration/data/threshold_grid.csv
/tmp/singularity-calibration/scripts/analyze_calibration.py /tmp/singularity-calibration/data/calibration_windows.csv /tmp/singularity-calibration/data/threshold_grid.csv /tmp/singularity-calibration/data
/tmp/singularity-calibration/scripts/analyze_repeat_offenders.py /tmp/singularity-calibration/work/calibration/scores /tmp/singularity-calibration/inventory/calibration_timestamps.txt /tmp/singularity-calibration/data --high 2.0 --low 0.3 --windows 8
/tmp/singularity-calibration/scripts/render_report.py
```

## Final concise summary

Conformance verdict: PASS across 24 real windows and 7,633,352 address comparisons. Maximum displayed deviations were 7.737e-15 alpha, 1.404e-08 intercept, and 1.168e-14 r2. Tolerance violations, level mismatches, and missing addresses were all zero.

Recommended constants: `threshold_high = 2` and `threshold_low = 0.3`. High 2.0 brings the sparse-address tail back into the feed, while low 0.3 keeps the dense-cluster tail selective. At the 20-per-tail cap, expected alerts per window are mean 19.7, median 20.0, with p10 to p90 of 14.0 to 25.0.

Sensitivity around the recommendation:

| High | Low   | Raw mean | Raw median | Raw p10 to p90 | Capped mean | Capped median | Capped p10 to p90 | Zero windows |
| ---- | ----- | -------- | ---------- | -------------- | ----------- | ------------- | ----------------- | ------------ |
| 1.75 | 0.3   | 63.2     | 62.0       | 51.0 to 74.1   | 25.2        | 25.0          | 24.0 to 27.0      | 0.0%         |
| 1.9  | 0.3   | 34.3     | 33.5       | 26.0 to 41.0   | 25.1        | 25.0          | 24.0 to 27.0      | 0.0%         |
| 2    | 0.25  | 16.8     | 17.0       | 11.0 to 23.0   | 16.5        | 17.0          | 11.0 to 22.0      | 0.0%         |
| 2    | 0.275 | 19.2     | 19.0       | 13.0 to 25.0   | 18.6        | 19.0          | 13.0 to 24.0      | 0.0%         |
| 2    | 0.3   | 20.8     | 20.0       | 14.0 to 26.0   | 19.7        | 20.0          | 14.0 to 25.0      | 0.0%         |
| 2    | 0.325 | 22.0     | 21.0       | 15.0 to 27.0   | 20.9        | 21.0          | 15.0 to 26.0      | 0.0%         |
| 2    | 0.35  | 41.2     | 27.0       | 17.9 to 40.1   | 26.4        | 26.0          | 17.9 to 36.1      | 0.0%         |
| 2.1  | 0.3   | 11.9     | 11.0       | 7.0 to 15.1    | 11.1        | 11.0          | 7.0 to 15.1       | 0.0%         |
| 2.25 | 0.3   | 6.7      | 6.0        | 4.0 to 8.0     | 6.0         | 6.0           | 4.0 to 8.0        | 0.0%         |
