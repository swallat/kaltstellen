#!/usr/bin/env bash
# Spec Q4: all `unsafe` code lives in src/process.rs.
# Backs up the compiler check (`#![deny(unsafe_code)]` in src/lib.rs), which
# an `#[allow]` could silence. Matches unsafe blocks, fns, impls, traits,
# extern blocks and `unsafe(...)` attributes, not the word in comments.
set -euo pipefail
cd "$(dirname "$0")/.."

pattern='\bunsafe[[:space:]]*(\{|\(|fn\b|impl\b|extern\b|trait\b)'
hits=$(grep -rnE "$pattern" --include='*.rs' --exclude-dir=target . \
    | grep -v '^\./src/process\.rs:' || true)

if [[ -n "$hits" ]]; then
    echo "unsafe code outside src/process.rs (spec Q4):"
    echo "$hits"
    exit 1
fi
echo "ok: unsafe only in src/process.rs"
