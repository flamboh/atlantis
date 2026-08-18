# MAAD conformance fixtures

Each `golden.json` is raw JSON emitted independently by the Haskell `MAAD`
oracle from `chris-misa/maad` at commit
`3ae75363d44e08faacdee186d4ff8906c6ccd06a`, with the sole source adjustment
`deltaQ = 1.0 / 8.0` (instead of `1.0 / 16.0`). The oracle was invoked with
`--input - --output - --format json --structure --spectrum --dimensions`,
feeding the corresponding address list on standard input; therefore committed
metadata has no machine-local input path. The integration test compares the
public Rust CLI JSON after normalizing only Rust/Haskell metadata naming.

The cases cover a clustered set, deterministic random addresses, a mixed set,
one-sided nearly-full-prefix pruning with ancestor propagation, and balanced
branching. Empty/sparse inputs and the 1024-address uniform-rounding case are
intentionally excluded.
