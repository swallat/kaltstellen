#!/usr/bin/env bash
# Writes a version into Cargo.toml and Cargo.lock, replacing the placeholder
# the repository carries. The release workflow runs it with the version from
# the tag before it builds; the change is never committed.
#
# Usage: scripts/set-version.sh <version>
set -euo pipefail
cd "$(dirname "$0")/.."

version=${1:?usage: set-version.sh <version>}
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z]+(\.[0-9A-Za-z]+)*)?$ ]]; then
    echo "::error::\"$version\" is not MAJOR.MINOR.PATCH[-pre]" >&2
    exit 1
fi

# The version line of [package] in Cargo.toml ...
sed -i '/^\[package\]/,/^\[/s/^version *= *"[^"]*"/version = "'"$version"'"/' Cargo.toml
# ... and the entry of the package in Cargo.lock (the line after its name).
sed -i '/^name = "kaltstellen"\r\{0,1\}$/{n;s/^version = "[^"]*"/version = "'"$version"'"/}' Cargo.lock

grep -q '^version = "'"$version"'"' Cargo.toml
grep -A1 '^name = "kaltstellen"' Cargo.lock | grep -q '^version = "'"$version"'"'
echo "version set to $version"
