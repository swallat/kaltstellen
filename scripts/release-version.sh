#!/usr/bin/env bash
# Decides what the release workflow builds and whether it publishes.
#
# Prints, in GITHUB_OUTPUT form:
#   version=<version to build>
#   publish=true|false
#   prerelease=true|false
#
# The tag sets the version: a pushed tag v<version> publishes that version,
# if the tag is MAJOR.MINOR.PATCH[-pre] and sits on a commit of main. A tag
# that breaks one of these fails the workflow. Everything else (manual run,
# pull request) is a dry run that builds version 0.0.0-dryrun and publishes
# nothing.
#
# Input: EVENT_NAME, REF_TYPE, REF_NAME as the workflow passes them, and
# optionally MAIN_REF (default origin/main).
set -euo pipefail
cd "$(dirname "$0")/.."

version=0.0.0-dryrun
publish=false
if [[ "${EVENT_NAME:-}" == "push" && "${REF_TYPE:-}" == "tag" ]]; then
    tag=${REF_NAME:-}
    if ! [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z]+(\.[0-9A-Za-z]+)*)?$ ]]; then
        echo "::error::tag \"$tag\" is not v<MAJOR.MINOR.PATCH> or v<MAJOR.MINOR.PATCH>-<pre>" >&2
        exit 1
    fi
    main_ref=${MAIN_REF:-origin/main}
    if ! git merge-base --is-ancestor HEAD "$main_ref"; then
        echo "::error::tag $tag is not on a commit of main" >&2
        exit 1
    fi
    version=${tag#v}
    publish=true
fi

prerelease=false
if [[ "$version" == *-* ]]; then
    prerelease=true
fi

echo "version=$version"
echo "publish=$publish"
echo "prerelease=$prerelease"
