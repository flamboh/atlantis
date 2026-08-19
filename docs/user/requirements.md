# Requirements

This document lists the required tools by concern. Install the tools for the parts that you use.

The repository has a `shell.nix` file that supplies every tool below except Git. With Nix, run `nix-shell` and skip the manual installation.

## Dashboard

The dashboard and all `bun run` commands need these tools:

| Tool    | Version                   | Source of truth |
| ------- | ------------------------- | --------------- |
| Git     | Current supported version | Git releases    |
| Bun     | 1.2.16                    | `package.json`  |
| Node.js | 22.16.0                   | `.node-version` |

Node.js is necessary even though Bun installs the packages. The development server runs under Node.js, and `bun install` needs Node.js on `PATH` to download the prebuilt SQLite driver. Without it, the install prints a `better-sqlite3` warning and the dashboard cannot open a database (see [Troubleshooting](troubleshooting.md)).

## Pipeline

Building a database with `scripts/netflow-db.sh` also needs the Rust toolchain:

| Tool   | Version                | Source of truth       |
| ------ | ---------------------- | --------------------- |
| rustup | Current stable version | rustup installation   |
| Rust   | 1.97.1                 | `rust-toolchain.toml` |
| cc     | gcc or clang           | Rust linker           |

rustup reads `rust-toolchain.toml` and installs the pinned Rust version automatically on the first build.

## nfdump fork

Processing nfcapd captures also needs the build tools for the pinned nfdump fork. CSV input does not.

- autoconf, automake, libtool
- flex (or lex) and bison (or yacc)
- make, gcc or clang, `pkg-config`
- Python 3 and `tar`

`./vendor/scripts/compile-nfdump.sh` checks for these tools before it builds and names any tool that is missing.

On Debian or Ubuntu, this command installs them:

```bash
sudo apt install build-essential autoconf automake libtool flex bison pkg-config python3
```

## Development

Contributors also need the Playwright browser dependencies for `bun run test:e2e`. `shell.nix` supplies them. Read [Development](../code/development.md) for the full workflow.
