# MAAD conformance

`scripts/local/validate_maad.py` is a small, local Rust-vs-Haskell comparison
for address sets. It accepts one or more `NAME=PATH` cases, where each path is
an external file containing one IPv4 address per line, or one IPv6 address per
line with `--ipv6`. The validator passes the same path, unchanged and in the
original order, to both implementations:

- Rust: `<rust> maad [--ipv6] <path>`
- Haskell: `<haskell> [--ipv6] --input <path> --output - --format json --structure --spectrum --dimensions`

`--ipv6` applies to every case in one run. Write IPv6 inputs as hexadecimal
groups (`::` compression is fine). The oracle misreads embedded IPv4 notation
such as `::ffff:192.0.2.1` without failing, so the validator rejects any IPv6
line that contains a dot.

It ignores the oracle's schema/input metadata differences, normalizes Rust's
`prefixLengths` and the oracle's `metadata.prefix_counts[*].pl` to the same
prefix-length list, then requires equal total addresses, prefix lengths, row
counts, and numeric rows. Numeric fields use independent absolute and relative
tolerances, both defaulting to `1e-10`.

## Pinned oracle

The Haskell oracle is pinned to
`chris-misa/maad@b7bbb7dd119f9e050cb74239b89e472ad58ec5db` on `main` ("filter for critical region when
estimating spectrum"). `main` carries the merged `atomic-full-sortout` branch.
The only source change made to that oracle checkout is `deltaQ`: Atlantis uses
`1/8` instead of the upstream `1/16`. The unpatched checkout is not comparable.
The goldens are generated from this patched checkout.

Build the oracle from that checkout by applying exactly that one-line change,
then run its build script in the supplied Nix shell:

```sh
git clone https://github.com/chris-misa/maad.git /tmp/maad
git -C /tmp/maad checkout b7bbb7dd119f9e050cb74239b89e472ad58ec5db
(cd /tmp/maad && git apply --unidiff-zero <<'PATCH'
diff --git a/MAAD.hs b/MAAD.hs
--- a/MAAD.hs
+++ b/MAAD.hs
@@ -93 +93 @@ deltaQ :: Double
-deltaQ = 1.0 / 16.0
+deltaQ = 1.0 / 8.0
PATCH
)
(cd /tmp/maad && nix-shell shell.nix --run './compile.sh')
```

If the checkout's GHC dependencies (including `wide-word`) are already
installed, `./compile.sh` can be run directly instead.

### Changes from the previous pin

The previous pin was `3ae75363d44e08faacdee186d4ff8906c6ccd06a`. For IPv4
address sets, the new pin produces the same prefix levels, structure rows,
and spectrum rows. The only estimator difference is in the last bits of some
floating-point sums, from a different summation order. The observable IPv4
changes are in the dimensions output, and Rust ports both:

- Dimension rows are ordered by `q` (`0`, `1`, `2`) instead of `1`, `0`, `2`.
- Each dimension row carries `sd`. For `q = 0` and `q = 2` it is the structure
  row's `sd` at that `q`; no rescaling is needed because `|q - 1| = 1`. For
  `q = 1` upstream emits `0`, because `D1` comes from an entropy regression
  with no variance estimate. Treat that `0` as "not estimated", not as a
  certain value.
- The spectrum keeps only rows from the critical region. With central
  differences `alpha = (tau[i+1] - tau[i-1]) / (2 q_step)` and
  `f = q alpha - tau`, the region runs from `q_min = min(0, min q <= 0 with
f > 0)` to `q_max = max(1, max q >= 1 with f > 0)`. Inside it, the alpha run
  must be non-increasing (upstream `a1 >= a2`) instead of strictly decreasing.
  Structure and dimensions are unchanged.

The new pin also adds IPv6 input (`--ipv6`), which Rust ports with the same
defaults: prefix lengths `/23`--`/64`, 128-bit prefixes, and a nearly-full
test of `log2(count) / (128 - pl)`. The IPv6 `q` grid and `full_threshold`
match IPv4. Upstream picked `/23` as the smallest RIR allocation size, and it
stops at `/64` because interface identifiers below `/64` are usually SLAAC or
privacy-random bits.

At the default `/23`--`/64` range the IPv6 nearly-full test is effectively
inert: a prefix at `/64` or shorter would need more than `2^60` addresses to
reach the threshold. Only the Rust unit test exercises IPv6 nearly-full pruning,
with a custom prefix range; the IPv6 goldens do not.

It also adds changes that Atlantis does not port:

- `--test` now runs a Hotelling T² test against a linear interpolation of the
  structure function, and `--compare-structure` compares against a
  precomputed structure CSV. Atlantis does not run hypothesis tests.
- Partition-function `q` values and the `UniformSet` generator changed. Neither
  is part of the compared output.

Build the Rust release binary from this repository:

```sh
./scripts/build_maad_fast.sh
```

## Running a comparison

Keep real or private address files outside the repository. They are inputs,
not fixtures, and must not be committed:

```sh
python3 scripts/local/validate_maad.py \
  --rust target/release/netflow-db \
  --haskell /tmp/maad/MAAD \
  private-window=/path/to/private/addresses.txt
```

Add `--ipv6` to compare IPv6 files. Add another file as another positional
`NAME=PATH` (or repeat `--case`). Use
`--abs-tol` and `--rel-tol` when a deliberate numerical comparison needs a
different tolerance. A passing case prints one compact summary; command,
input, JSON, row-count, metadata, or numeric mismatches return nonzero.

Pass `--weighted` to compare measure-weighted MAAD. Each case is then an
`ADDR,MEASURE` CSV with one row per distinct address and a finite, positive
measure: the oracle keeps only the first row for a repeated address, while Rust
sums them, so the validator rejects duplicates. Rust runs `netflow-db maad
--weighted` and the oracle runs with `--csv --meas-col 1`. Only structure and
dimensions are compared, because the weighted estimator does not emit a
spectrum. Combine it with `--ipv6` for IPv6 CSVs. Every summary line reports
`max_abs_dtau` and `max_abs_ddim`.

## Known edge cases

The Haskell executable cannot produce JSON for an empty set, a singleton, or a
set whose every prefix is filtered at the default range (`/8`--`/24` for IPv4,
`/23`--`/64` for IPv6). An empty
set fails when the oracle reads its address family; the other cases call
`foldl1` on an empty list after filtering. All exit nonzero. Rust returns an
empty result for these inputs. The validator reports the Haskell command
failure, so omit such cases from a passing conformance run or treat that
failure as the expected oracle limitation.

Rust treats alphas within its `1e-12` grid epsilon as ties, so a sub-epsilon
increase keeps the spectrum run going where upstream's exact `a1 >= a2` stops.
Upstream's exact test reacts to last-bit noise in `tau`: on a perfectly linear
structure curve, Haskell stops after 19 rows and Rust keeps all 30 tied rows.
The goldens and 20 real windows match exactly with this tolerance, and so do
tests with an exact comparison. Row counts remain exact in the validator, so
use cases with meaningful spectrum curvature when expecting a pass.

There is intentionally no synthetic-data generator, scheduled CI job, or
persisted report format. Keep sensitive and private address sets local.
