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

# Handle LD_LIBRARY_PATH
case ":${LD_LIBRARY_PATH:=}:" in
    *:"{WASMEDGE_LIB_DIR}":*)
        ;;
    *)
        # Prepending library path
        export LD_LIBRARY_PATH="{WASMEDGE_LIB_DIR}:${LD_LIBRARY_PATH}"
        ;;
esac