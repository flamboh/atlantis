# MAAD conformance fixtures

Each `golden.json` is raw JSON emitted independently by the Haskell `MAAD`
oracle from `chris-misa/maad` at commit
`b7bbb7dd119f9e050cb74239b89e472ad58ec5db` (`main`), with the sole source adjustment
`deltaQ = 1.0 / 8.0` (instead of `1.0 / 16.0`). The oracle was invoked with
`--input - --output - --format json --structure --spectrum --dimensions`,
feeding the corresponding address list on standard input; therefore committed
metadata has no machine-local input path. The integration test compares the
public Rust CLI JSON after normalizing only Rust/Haskell metadata naming.

The cases cover a clustered set, deterministic random addresses, a mixed set,
one-sided nearly-full-prefix pruning with ancestor propagation, and balanced
branching. Empty/sparse inputs and the 1024-address uniform-rounding case are
intentionally excluded.

The `ipv6-*` cases are synthetic IPv6 sets: `ipv6-clustered` nests random
subnets inside random sites and allocations, and `ipv6-mixed` combines
low-numbered hosts in a few `/64`s with scattered random addresses. Their
goldens add `--ipv6` to the oracle invocation, and the integration test runs
`netflow-db maad --ipv6`. Inputs are written as eight hexadecimal groups
because the oracle misreads embedded IPv4 notation such as `::ffff:192.0.2.1`.
