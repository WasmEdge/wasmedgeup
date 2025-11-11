# wasmedgeup shell setup for Fish
# The {WASMEDGE_BIN_DIR} placeholder is expected to be replaced by the actual WasmEdge bin path.

if not contains "{WASMEDGE_BIN_DIR}" $PATH
    # Prepending path
    set -gx PATH "{WASMEDGE_BIN_DIR}" $PATH
end

# Configure WasmEdge plugins
if not set -q WASMEDGE_PLUGIN_PATH
    set -gx WASMEDGE_PLUGIN_PATH "{WASMEDGE_PLUGIN_DIR}"
end

# Handle library paths for different platforms
switch (uname)
    case Linux
        if not contains "{WASMEDGE_LIB_DIR}" $LD_LIBRARY_PATH
            if set -q LD_LIBRARY_PATH
                set -gx LD_LIBRARY_PATH "{WASMEDGE_LIB_DIR}" $LD_LIBRARY_PATH
            else
                set -gx LD_LIBRARY_PATH "{WASMEDGE_LIB_DIR}"
            end
        end
    case Darwin
        if not contains "{WASMEDGE_LIB_DIR}" $DYLD_LIBRARY_PATH
            if set -q DYLD_LIBRARY_PATH
                set -gx DYLD_LIBRARY_PATH "{WASMEDGE_LIB_DIR}" $DYLD_LIBRARY_PATH
            else
                set -gx DYLD_LIBRARY_PATH "{WASMEDGE_LIB_DIR}"
            end
        end
end
