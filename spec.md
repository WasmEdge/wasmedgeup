## Spec

`wasmedgeup` will be a self-contained and static binary written in Rust that can:

1. **Install** and **remove** specific versions of the WasmEdge runtime.
2. **List** available WasmEdge runtime versions.
3. **Install**, **list**, and **remove** WasmEdge plugins.
4. Handle cross-OS and cross-architecture detection automatically, unless overridden by explicit flags.
5. Use checksum to ensure safe downloads.

### WasmEdge runtime

#### Commands

wasmedgeup should have the following commands:

1. `install`: Installs a specified (or latest) WasmEdge runtime version.
2. `list`: Lists all available WasmEdge releases.
3. `remove`: Uninstalls a specific version of WasmEdge from the system, removing installed files.
4. `help`: Shows a usage overview or help message for each subcommand.

##### Command `Install`

###### Arguments

1. `install latest`: Installs the latest WasmEdge released version.
2. `install <specific version, e.g. 0.14.1>`: Installs the specified version, e.g. `0.14.1`, `0.14.1-rc.1`, etc.

###### Options

- `-p`, `--path`
  - Description: Set the installed location
  - Usage: `--path /usr/local`
  - Default: `$HOME/.wasmedge`
- `-t`, `--tmpdir`
  - Description: Set the temporary directory for staging downloaded assets
  - Usage: `--tmpdir /tmp`
  - Default: `/tmp`
- `-o`, `--os`
  - Description: Overwrite the OS detection. If omitted, `wasmedgeup` auto-detects.
  - Usage: `--os Darwin`
  - Possible values: `Linux`, `Darwin` (macOS), `Windows`, or distro-specific like `Ubuntu`.
- `-a`, `--arch`
  - Description: Overwrite the ARCH detection. If omitted, `wasmedgeup` auto-detects.
  - Usage: `--arch aarch64`
  - Possible values: `x86_64`, `arm64`, `aarch64` (where `arm64` is synonymous with `aarch64`).

#### Global Options

1. `-v`, `--version`: Prints wasmedgeup installer version (not the runtime)
2. `-V`, `--verbose`: Enables verbose output
3. `-q`, `--quite`: Disables progress output

#### Internal Behavior / OS & ARCH Detection

When no OS or ARCH flags are provided, `wasmedgeup` should detect the operating systems and the architectures automatically.

1. ARCH: "x86_64", "arm64", "aarch64". Please note that "arm64" equals "aarch64".
2. OS: Typically one of "Ubuntu", "Linux" (generic for most distributions besides Ubuntu), "Darwin" (macOS), "Windows"

If ARCH and OS are not matched to the above list, `wasmedgeup` should raise an error and refuse to proceed.

#### Examples

```bash
# List versions
$ wasmedgeup list
0.15.0 <- latest
0.14.1
0.14.1-rc.1
0.14.1-beta.2
0.14.1-alpha.3
0.14.0
0.13.5

# Install latest
$ wasmedgeup install latest
... installing 0.15.0

# Install a specific version
$ wasmedgeup install 0.13.5-rc.1
... installing 0.13.5-rc.1

# Override OS and ARCH
$ wasmedgeup install 0.15.0 -p /usr/local -t /tmp -o Darwin -a aarch64
... installing 0.15.0 with the following config: (Darwin, aarch64) to /usr/local via /tmp

# Ubuntu + x86_64
$ wasmedgeup install latest --path /usr/local --tmpdir /tmp --os Ubuntu --arch x86_64
... installing latest(0.15.0) with the following config: (Ubuntu, x86_64) to /usr/local via /tmp
```

### WasmEdge plugins

Just as with the runtime installation, `wasmedgeup` should manage plugins in a uniform way.

#### Commands `plugin`

1. `install`: Installs the specific WasmEdge plugins.
2. `list`: Lists all available WasmEdge plugins according to the installed WasmEdge runtime version
3. `remove`: Uninstalls the specific WasmEdge plugins

##### Command `install`

1. `install package`: Install the given plugin, e.g. `wasi-nn-ggml`
2. `install package_1 package_2 ...`: Install multiple given plugins, split by space
3. `install package@version`: Install the given plugin with specific version

- Steps:  
  1. Checks the currently installed WasmEdge runtime version (e.g., `0.15.0`).
  2. Retrieves the plugin manifest JSON from links, described below.
  3. Resolves the best matching plugin binaries for the user’s OS, ARCH, and runtime version.
  4. Downloads, verifies, and installs them into the WasmEdge plugin directory (e.g., `$HOME/.wasmedge/plugins`).

##### Command `remove`

Just remove the installed plugins.

1. `remove package`: Remove the given plugin, e.g. `wasi-nn-ggml`
2. `remove package_1 package_2 ...`: Remove multiple given plugins, split by space
3. `remove package@version`: remove the given plugin with specific version

##### Command `list`

Show all avaliable plugins. We will provide several manifests for it.
Assuming there are two repositories called `wasmedge/cpp_plugins` and `wasmedge/rust_plugins`.
Both of them provide a branch or tag called `latest`. So the installer can always retrieve the list via the following two links:

1. `https://github.com/WasmEdge/cpp_plugins/releases/download/latest/version.json`
2. `https://github.com/WasmEdge/rust_plugins/releases/download/latest/version.json`

The content of these version.json provide:

```json
{
   "maintained": ["0.14.1", "0.15.0"]
   "deprecated": ["0.13.0", "0.13.1", "0.13.2", "0.13.3", "0.13.4", "0.13.5", "0.14.0"]
}
```

Assuming we will have the following branches/tags called `<wasmedge_runtime_versions>`. So the installer can always retrieve the plugin information via the following links:

1. `https://github.com/WasmEdge/cpp_plugins/releases/download/0.15.0/version.json`
2. `https://github.com/WasmEdge/rust_plugins/releases/download/0.14.1/version.json`

The content of these version.json provide:

```json
{
    "<plugin_name>": {
        "<version>": {
            "deps": []
            "platform": [
                "manylinux_2_28_x86_64",
                "manylinux_2_28_aarch64",
                "ubuntu20_04_x86_64",
                "ubuntu20_04_aarch64",
                "darwin_x86_64",
                "darwin_arm64",
                "windows_x86_64"
            ]
        },
        "<version>": {
            "deps": []
            "platform": [
                "manylinux_2_28_x86_64",
                "manylinux_2_28_aarch64",
                "ubuntu20_04_x86_64",
                "ubuntu20_04_aarch64",
                "darwin_x86_64",
                "darwin_arm64",
                "windows_x86_64"
            ]
        }
    },
    "wasi-nn-ggml": {
        "0.1.18": {
            "deps": []
            "platform": [
                "manylinux_2_28_x86_64",
                "manylinux_2_28_aarch64",
                "ubuntu20_04_x86_64",
                "ubuntu20.04_x86_64",
                "ubuntu20_04_aarch64",
                "windows_x86_64"
            ]
        }
    }
}
```

Assuming the plugin assets has their own release tag like the following format:

1. `https://github.com/WasmEdge/cpp_plugins/releases/download/<plugin_name>-<wasmedge_runtime_version>-<plugin_version>`
2. `https://github.com/WasmEdge/rust_plugins/releases/download/<plugin_name>-<wasmedge_runtime_version>-<plugin_version>`

So the installer can access these assets via composing the link:

1. `https://github.com/WasmEdge/cpp_plugins/releases/download/wasi-nn-ggml-0.15.0-0.1.18`
2. `https://github.com/WasmEdge/rust_plugins/releases/download/wasi-xxx-0.14.1-0.6.4`

#### Examples for plugins

```bash
# List plugins, assuming installed wasmedge 0.15.0, os is Darwin, and arch is aarch64
wasmedgeup plugin list
wasi-nn-ggml 0.1.18
wasi-nn-ggml 0.1.19
wasi-windows-ext 0.2.0 [Not compatible, provides (Windows, x86_64)]
...

# Install plugins
wasmedgeup plugin install wasmedge-tensorflow-lite # WasmEdge TensorFlow Lite plugin
wasmedgeup plugin install wasmedge-image@0.2.0 # WasmEdge Image plugin
wasmedgeup plugin install wasi-nn-ggml wasi-nn-whisper # WasmEdge WASI NN plugins

# Remove plugins
wasmedgeup plugin remove wasmedge-tensorflow-lite
wasmedgeup plugin remove wasmedge-image@0.2.0
wasmedgeup plugin remove wasi-nn-ggml wasi-nn-whisper
```
