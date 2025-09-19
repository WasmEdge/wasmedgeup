# wasmedgeup shell setup for Nushell
# The {WASMEDGE_BIN_DIR} placeholder is expected to be replaced by the actual WasmEdge bin path.

use std/util "path add"

path add "{WASMEDGE_BIN_DIR}"

path add "{WASMEDGE_LIB_DIR}" LD_LIBRARY_PATH