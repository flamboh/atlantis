# Requirements

Install only what your path needs. On NixOS, `nix-shell` supplies every tool below.

## Dashboard

| Tool    | Version | Pinned in       |
| ------- | ------- | --------------- |
| Git     | Any     |                 |
| Bun     | 1.3.11  | `package.json`  |
| Node.js | 24.18.1 | `.node-version` |

Node.js must be on `PATH` when you run `bun install`, or the SQLite driver fails to install (see [Troubleshooting](troubleshooting.md)).

## Docker pipeline

Git and Docker. The image supplies everything else.

## Native pipeline

- [rustup](https://rustup.rs). It installs the pinned Rust version from `rust-toolchain.toml` on the first build.
- A C compiler (gcc or clang).

## Native nfdump fork

Needed for nfcapd captures on the native path, not for CSV. On Debian or Ubuntu:

```bash
sudo apt install build-essential autoconf automake libtool flex bison pkg-config python3
```

`./vendor/scripts/compile-nfdump.sh` names any missing tool.

## Development

Contributors also need the Playwright browser dependencies for `bun run test:e2e`. See [Development](../code/development.md).
