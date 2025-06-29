# wasmedgeup shell setup for Fish
# The {WASMEDGE_BIN_DIR} placeholder is expected to be replaced by the actual WasmEdge bin path.

if not contains "{WASMEDGE_BIN_DIR}" $PATH
    # Prepending path
    set -gx PATH "{WASMEDGE_BIN_DIR}" $PATH
end
