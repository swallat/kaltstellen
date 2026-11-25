#!/usr/bin/env bash
# Checks the GitHub settings that the automated maintenance depends on
# (docs/maintenance.md) and, with --apply, fixes what the API allows.
#
#   scripts/setup-maintenance.sh            report only, changes nothing
#   scripts/setup-maintenance.sh --apply    also fix what can be fixed
#
# Needs the GitHub CLI (gh), signed in as a repository admin. Safe to run
# again: it only sets what is not set yet.
set -euo pipefail

apply=false
case "${1:-}" in
    "") ;;
    --apply) apply=true ;;
    *) echo "usage: $0 [--apply]" >&2; exit 2 ;;
esac

command -v gh > /dev/null || { echo "The GitHub CLI (gh) is not installed." >&2; exit 1; }
repo=${GH_REPO:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}
echo "Repository: $repo"
todo=0

ok()   { echo "  ok       $*"; }
fixed() { echo "  fixed    $*"; }
manual() { echo "  MANUAL   $*"; todo=$((todo + 1)); }

echo "Repository settings"
settings=$(gh api "repos/$repo")
if [ "$(jq -r .allow_auto_merge <<< "$settings")" = "false" ]; then
    ok "GitHub auto-merge is off (Renovate merges after all checks are green)"
else
    manual "Allow auto-merge is on; turn it off under Settings > General > Pull Requests"
fi
if [ "$(jq -r .delete_branch_on_merge <<< "$settings")" = "true" ]; then
    ok "merged branches are deleted"
elif $apply; then
    gh api -X PATCH "repos/$repo" -F delete_branch_on_merge=true > /dev/null
    fixed "merged branches are now deleted"
else
    manual "run with --apply to delete merged branches automatically"
fi

echo "Security features"
if gh api "repos/$repo/vulnerability-alerts" > /dev/null 2>&1; then
    ok "Dependabot alerts are on (Renovate reads them)"
elif $apply; then
    gh api -X PUT "repos/$repo/vulnerability-alerts" > /dev/null
    fixed "Dependabot alerts are now on"
else
    manual "run with --apply to turn on Dependabot alerts"
fi
if [ "$(jq -r '.security_and_analysis.dependabot_security_updates.status // "disabled"' <<< "$settings")" = "disabled" ]; then
    ok "Dependabot security updates are off (Renovate does this job)"
else
    manual "turn off Dependabot security updates under Settings > Code security"
fi

echo "Labels"
declare -A labels=(
    [dependencies]="0366d6|Dependency updates"
    [security]="d73a4a|Security fix"
    [needs-review]="fbca04|Not merged automatically, needs a person"
    [vulnerability]="b60205|Open vulnerability found by the weekly scan"
    [release-hint]="5319e7|Security fixes wait for a release"
)
existing=$(gh api --paginate "repos/$repo/labels?per_page=100" --jq '.[].name')
for name in "${!labels[@]}"; do
    if grep -qxF "$name" <<< "$existing"; then
        ok "label $name"
    elif $apply; then
        gh label create "$name" --repo "$repo" --color "${labels[$name]%%|*}" \
            --description "${labels[$name]#*|}" > /dev/null
        fixed "label $name created"
    else
        manual "run with --apply to create the label $name"
    fi
done

echo "Ruleset on the default branch"
gh api "repos/$repo/rules/branches/$(jq -r .default_branch <<< "$settings")" \
    --jq '.[] | select(.type=="required_status_checks") | .parameters.required_status_checks[].context' \
    | sed 's/^/  required check: /' || true
echo "  (the ARM64 job is not required on purpose: Renovate waits for it anyway)"

echo "Things only the GitHub web pages can do"
if [ "$(gh api "repos/$repo/issues?state=all&creator=renovate%5Bbot%5D&per_page=1" --jq length)" = "1" ]; then
    ok "the Renovate app has worked in this repository"
else
    manual "install the Renovate app: https://github.com/apps/renovate (Only select repositories)"
fi
echo "  MANUAL   check that the Claude GitHub App has write access to this"
echo "           repository and may run workflows (Actions: write):"
echo "           https://github.com/apps/claude/installations/select_target"
todo=$((todo + 1))

echo
if [ "$todo" -eq 0 ]; then echo "Everything is in place."; else echo "$todo item(s) need attention."; fi
