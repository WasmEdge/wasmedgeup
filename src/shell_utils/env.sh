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
