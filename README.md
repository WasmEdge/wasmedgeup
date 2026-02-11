# WasmEdgeUp

`wasmedgeup` is a command-line tool for managing WasmEdge runtime installations and plugins across different operating systems and architectures.

## Features

- **Install** and **remove** specific versions of the WasmEdge runtime
- **List** available WasmEdge runtime versions
- **Install**, **list**, and **remove** WasmEdge plugins
- Automatic cross-OS and cross-architecture detection
- Checksum verification for secure downloads

## Installation

### Option 1: Install from crates.io

If you have the Rust toolchain installed ([rustup.rs](https://rustup.rs)):

```sh
cargo install wasmedgeup
```

### Option 2: Pre-built binaries

Download a pre-built binary from the [GitHub releases page](https://github.com/WasmEdge/wasmedgeup/releases).

| Target | OS | Arch |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | Linux | x86_64 |
| `x86_64-unknown-linux-musl` | Linux | x86_64 (static) |
| `aarch64-unknown-linux-gnu` | Linux | ARM64 |
| `aarch64-unknown-linux-musl` | Linux | ARM64 (static) |
| `x86_64-apple-darwin` | macOS | x86_64 |
| `aarch64-apple-darwin` | macOS | Apple Silicon |
| `x86_64-pc-windows-msvc` | Windows | x86_64 |

```sh
# Example for Linux x86_64
curl -LO https://github.com/WasmEdge/wasmedgeup/releases/latest/download/wasmedgeup-x86_64-unknown-linux-gnu.tgz
tar xzf wasmedgeup-x86_64-unknown-linux-gnu.tgz
```

### Option 3: Build from source

```sh
git clone https://github.com/WasmEdge/wasmedgeup.git
cd wasmedgeup
cargo build --release
```

The binary will be at `target/release/wasmedgeup`. Move it somewhere on your `PATH` to use it.

## Usage

Please refer to the [specification](spec.md) for detailed usage instructions.

## Release Process

Releases are automated via [Knope](https://knope.tech) and GitHub Actions.

1. **Push to `master`** — the `prepare-release` workflow runs `knope prepare-release`, which:
   - Scans conventional commits since the last release
   - Bumps the version in `Cargo.toml` / `Cargo.lock`
   - Updates `CHANGELOG.md`
   - Opens (or updates) a PR from the `release` branch to `master`

2. **Merge the release PR** — the `release` workflow builds artifacts for all platforms, then runs `knope release` to publish the GitHub release with attached binaries. The crate is then published to crates.io.

If a push contains no releasable changes (e.g. `chore(deps):` bumps only), the prepare-release step exits gracefully and no PR is created.

### Prerequisites

- A **Personal Access Token** stored as the repository secret `PAT` with `contents: write` and `pull-requests: write` scopes. This is required so the release PR triggers CI checks.
- **Trusted publishing** configured on crates.io for the `wasmedgeup` crate (no secret needed — uses GitHub OIDC).

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.
