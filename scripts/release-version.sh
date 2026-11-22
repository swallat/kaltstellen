#!/usr/bin/env bash
# Decides what the release workflow builds and whether it publishes.
#
# Prints, in GITHUB_OUTPUT form:
#   version=<version from Cargo.toml>
#   publish=true|false
#   prerelease=true|false
#
# It publishes only for a pushed tag v<version> that
#   - equals the version in Cargo.toml (the updater refuses an executable
#     whose --version differs from its release), and
#   - sits on a commit of main.
# A tag that breaks one of these fails the workflow. Everything else (manual
# run, pull request) is a dry run.
#
# Input: EVENT_NAME, REF_TYPE, REF_NAME as the workflow passes them, and
# optionally MAIN_REF (default origin/main).
set -euo pipefail
cd "$(dirname "$0")/.."

version=$(sed -n '/^\[package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n 1 | tr -d '\r')
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
    echo "::error::version \"$version\" in Cargo.toml is not MAJOR.MINOR.PATCH[-pre]" >&2
    exit 1
fi

publish=false
if [[ "${EVENT_NAME:-}" == "push" && "${REF_TYPE:-}" == "tag" ]]; then
    if [[ "${REF_NAME:-}" != "v$version" ]]; then
        echo "::error::tag ${REF_NAME:-} does not match version $version in Cargo.toml (expected v$version)" >&2
        exit 1
    fi
    main_ref=${MAIN_REF:-origin/main}
    if ! git merge-base --is-ancestor HEAD "$main_ref"; then
        echo "::error::tag ${REF_NAME} is not on a commit of main" >&2
        exit 1
    fi
    publish=true
fi

prerelease=false
if [[ "$version" == *-* ]]; then
    prerelease=true
fi

echo "version=$version"
echo "publish=$publish"
echo "prerelease=$prerelease"
