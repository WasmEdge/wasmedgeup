# wasmedgeup shell setup for Fish
# The {WASMEDGE_BIN_DIR} placeholder is expected to be replaced by the actual WasmEdge bin path.

if not contains "{WASMEDGE_BIN_DIR}" $PATH
    # Prepending path
    set -gx PATH "{WASMEDGE_BIN_DIR}" $PATH
end

# Handle LD_LIBRARY_PATH
if not contains "{WASMEDGE_LIB_DIR}" $LD_LIBRARY_PATH
    # Prepending library path
    set -gx LD_LIBRARY_PATH "{WASMEDGE_LIB_DIR}" $LD_LIBRARY_PATH
end