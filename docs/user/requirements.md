# Requirements

This document lists the required tools by concern. Install the tools for the parts that you use.

For native setup, the repository has a `shell.nix` file that supplies every development tool below, including Git and a C compiler. With Nix, run `nix-shell` and skip the manual installation. You still need Git on the host to clone the repository before the shell is available.

## Docker pipeline

The Docker pipeline path needs only these host tools:

- Git
- Docker Engine or Docker Desktop

The first run builds the image and downloads Ubuntu packages, the pinned Rust toolchain, Rust crates, and the pinned nfdump fork. You do not need to initialize Git submodules or install a compiler on the host.

Docker covers the pipeline only. Run the dashboard natively with the tools in the next section.

## Dashboard

The dashboard and all `bun run` commands need these tools:

| Tool    | Version                   | Source of truth |
| ------- | ------------------------- | --------------- |
| Git     | Current supported version | Git releases    |
| Bun     | 1.2.16                    | `package.json`  |
| Node.js | 22.16.0                   | `.node-version` |

Node.js is necessary even though Bun installs the packages. The development server runs under Node.js, and `bun install` needs Node.js on `PATH` to download the prebuilt SQLite driver. Without it, the install prints a `better-sqlite3` warning and the dashboard cannot open a database (see [Troubleshooting](troubleshooting.md)).

## Native pipeline

Building a database with `scripts/netflow-db.sh` also needs the Rust toolchain:

| Tool   | Version                | Source of truth       |
| ------ | ---------------------- | --------------------- |
| rustup | Current stable version | rustup installation   |
| Rust   | 1.97.1                 | `rust-toolchain.toml` |
| cc     | gcc or clang           | Rust linker           |

rustup reads `rust-toolchain.toml` and installs the pinned Rust version automatically on the first build.

## Native nfdump fork

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
