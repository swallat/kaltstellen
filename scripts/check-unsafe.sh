#!/usr/bin/env bash
# Spec Q4: all `unsafe` code of the tool lives in src/process.rs and the
# Win32 wrapper tree src/win32/ (widened in WP7). The test
# fixtures in tests/fixtures/ are exempt (decision 2026-11-22: never shipped).
# Backs up the compiler check (`#![deny(unsafe_code)]` in src/lib.rs), which
# an `#[allow]` could silence. Matches unsafe blocks, fns, impls, traits,
# extern blocks and `unsafe(...)` attributes, not the word in comments.
set -euo pipefail
cd "$(dirname "$0")/.."

pattern='\bunsafe[[:space:]]*(\{|\(|fn\b|impl\b|extern\b|trait\b)'
hits=$(grep -rnE "$pattern" --include='*.rs' --exclude-dir=target . \
    | grep -v -e '^\./src/process\.rs:' -e '^\./src/win32/' -e '^\./tests/fixtures/' || true)

if [[ -n "$hits" ]]; then
    echo "unsafe code outside src/process.rs and src/win32/ (spec Q4):"
    echo "$hits"
    exit 1
fi
echo "ok: unsafe only in src/process.rs and src/win32/"
