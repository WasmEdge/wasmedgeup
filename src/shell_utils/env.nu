# wasmedgeup shell setup for Nushell
# The {WASMEDGE_BIN_DIR} placeholder is expected to be replaced by the actual WasmEdge bin path.

use std/util "path add"

path add "{WASMEDGE_BIN_DIR}"

# Add library path based on platform
if (uname) == "Linux" {
    path add "{WASMEDGE_LIB_DIR}" LD_LIBRARY_PATH
} else if (uname) == "Darwin" {
    path add "{WASMEDGE_LIB_DIR}" DYLD_LIBRARY_PATH
}

# Configure WasmEdge plugins
if ($env | get WASMEDGE_PLUGIN_PATH? | is-empty) {
    $env.WASMEDGE_PLUGIN_PATH = "{WASMEDGE_PLUGIN_DIR}"
}
