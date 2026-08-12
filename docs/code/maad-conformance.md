# MAAD conformance

`scripts/local/validate_maad.py` is a small, local Rust-vs-Haskell comparison
for address sets. It accepts one or more `NAME=PATH` cases, where each path is
an external file containing one IPv4 address per line. The validator passes the
same path, unchanged and in the original order, to both implementations:

- Rust: `<rust> maad <path>`
- Haskell: `<haskell> --input <path> --output - --format json --structure --spectrum --dimensions`

It ignores the oracle's schema/input metadata differences, normalizes Rust's
`prefixLengths` and the oracle's `metadata.prefix_counts[*].pl` to the same
prefix-length list, then requires equal total addresses, prefix lengths, row
counts, and numeric rows. Numeric fields use independent absolute and relative
tolerances, both defaulting to `1e-10`.

## Pinned oracle

The Haskell oracle is pinned to
`chris-misa/maad@3ae75363d44e08faacdee186d4ff8906c6ccd06a`. The only source
change made to that oracle checkout is `deltaQ`: Atlantis uses `1/8` instead
of the upstream `1/16`. The unpatched checkout is not comparable.

Build the oracle from that checkout by applying exactly that one-line change,
then run its build script in the supplied Nix shell:

```sh
git clone https://github.com/chris-misa/maad.git /tmp/maad
git -C /tmp/maad checkout 3ae75363d44e08faacdee186d4ff8906c6ccd06a
(cd /tmp/maad && git apply --unidiff-zero <<'PATCH'
diff --git a/MAAD.hs b/MAAD.hs
--- a/MAAD.hs
+++ b/MAAD.hs
@@ -66 +66 @@ deltaQ :: Double
-deltaQ = 1.0 / 16.0
+deltaQ = 1.0 / 8.0
PATCH
)
(cd /tmp/maad && nix-shell shell.nix --run './compile.sh')
```

If the checkout's GHC dependencies are already installed, `./compile.sh` can
be run directly instead.

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

Add another file as another positional `NAME=PATH` (or repeat `--case`). Use
`--abs-tol` and `--rel-tol` when a deliberate numerical comparison needs a
different tolerance. A passing case prints one compact summary; command,
input, JSON, row-count, metadata, or numeric mismatches return nonzero.

## Known edge cases

The Haskell executable cannot produce JSON for an empty set, a singleton, or a
set whose every prefix is filtered at the default `/8`--`/24` range: after
filtering, it calls `foldl1` on an empty list and exits nonzero. Rust returns an
empty result for these inputs. The validator reports the Haskell command
failure, so omit such cases from a passing conformance run or treat that
failure as the expected oracle limitation.

Rust also intentionally treats alpha decreases at or below its `1e-12` grid
epsilon as non-decreasing when building the spectrum. Consequently, a
sub-epsilon spectrum turn can produce fewer Rust rows than Haskell; row counts
remain exact in the validator, so use cases with meaningful spectrum curvature
when expecting a pass.

There is intentionally no synthetic-data generator, scheduled CI job, or
persisted report format. Keep sensitive and private address sets local.
