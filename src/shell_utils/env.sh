#!/bin/sh
# wasmedgeup shell setup
# The {WASMEDGE_BIN_DIR} placeholder is expected to be replaced by the actual WasmEdge bin path.

# affix colons on either side of $PATH to simplify matching
case ":${PATH}:" in
    *:"{WASMEDGE_BIN_DIR}":*)
        ;;
    *)
        # Prepending path
        export PATH="{WASMEDGE_BIN_DIR}:$PATH"
        ;;
esac

# Handle platform-specific library paths
case $(uname) in
    Linux)
        # Prepending library path for Linux
        export LD_LIBRARY_PATH="{WASMEDGE_LIB_DIR}:${LD_LIBRARY_PATH}"
        ;;
    Darwin)
        # Prepending library path for macOS
        export DYLD_LIBRARY_PATH="{WASMEDGE_LIB_DIR}:${DYLD_LIBRARY_PATH}"
        ;;
esac

# Configure WasmEdge plugins
: "${WASMEDGE_PLUGIN_PATH}"
if [ -z "${WASMEDGE_PLUGIN_PATH}" ]; then
    export WASMEDGE_PLUGIN_PATH="{WASMEDGE_PLUGIN_DIR}"
fi
