# WasmEdgeUp

Note: This project is still in development and not yet ready for use.

`wasmedgeup` is a command-line tool for managing WasmEdge runtime installations and plugins across different operating systems and architectures.

## Features

- **Install** and **remove** specific versions of the WasmEdge runtime
- **List** available WasmEdge runtime versions
- **Install**, **list**, and **remove** WasmEdge plugins
- Automatic cross-OS and cross-architecture detection
- Checksum verification for secure downloads

## Installation

Requires the Rust toolchain (Cargo). If you don't have Cargo installed, install Rust via rustup: [rustup.rs](https://rustup.rs)

### 1) Clone and build

```sh
git clone https://github.com/WasmEdge/wasmedgeup.git
cd wasmedgeup
cargo build --release
```

### 2) Install the binary to your PATH

#### Linux

```sh
mkdir -p "$HOME/.local/bin"
cp target/release/wasmedgeup "$HOME/.local/bin/"
echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.bashrc"
# For zsh users:
echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.zshrc"
```

Open a new terminal (or run `source ~/.bashrc` / `source ~/.zshrc`) and verify:

```sh
wasmedgeup --help
```

#### macOS

Option A: user-local install

```sh
mkdir -p "$HOME/.local/bin"
cp target/release/wasmedgeup "$HOME/.local/bin/"
echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$HOME/.zshrc"
```

Option B: system-wide (may require sudo)

```sh
sudo cp target/release/wasmedgeup /usr/local/bin/
```

Then open a new terminal and run:

```sh
wasmedgeup --help
```

#### Windows (PowerShell)

```powershell
cargo build --release
New-Item -Force -ItemType Directory "$env:USERPROFILE\.cargo\bin" | Out-Null
Copy-Item -Force target\release\wasmedgeup.exe "$env:USERPROFILE\.cargo\bin\"

# Ensure the directory is on PATH (per-user)
[Environment]::SetEnvironmentVariable(
  'Path',
  [Environment]::GetEnvironmentVariable('Path','User') + ";$env:USERPROFILE\\.cargo\\bin",
  'User'
)
# Restart your terminal, then:
wasmedgeup --help
```

## Usage

Please refer to the [specification](spec.md) for detailed usage instructions.

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.
