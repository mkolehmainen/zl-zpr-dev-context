#!/usr/bin/env bash
#
# Turn GitHub Actions off (or back on) for every fork in the workspace.
#
# Why this exists: none of the CI in the `mkolehmainen/zl-zpr-*` forks can run.
# `pr-notify.yml` needs `SLACK_ALERT_WEBHOOK_URL` and every caller of the shared
# `rust-build-test.yml` / `rust-test.yml` / `go-build-test.yml` reusable workflows
# needs `ZPR_CICD_RO_TOKEN`, which the reusable workflow declares *required* --
# and a fork inherits no secrets. Those jobs fail in about two seconds, before
# any step runs, which leaves every PR permanently red for a reason no PR can
# fix. See "Definition of done" in ../SKILL.md.
#
# This flips the repository-level Actions switch rather than deleting workflow
# files, so nothing in `.github/` diverges from upstream and no future upstream
# sync conflicts there. It is reversible: `--enable` puts it back.
#
# Usage:
#   ./fork-ci.sh --status          # report only, change nothing (default)
#   ./fork-ci.sh --disable         # turn Actions off everywhere
#   ./fork-ci.sh --enable          # turn Actions back on everywhere
#   ./fork-ci.sh --disable --dry-run
#
# Requires: `gh` authenticated with a token carrying the `repo` scope. Writing
# this setting needs admin on the repository, which you have on your own forks.
#
# Side effects of disabling, both expected:
#   - `zl-zpr-core` has `dependabot/github_actions/*` branches, so Dependabot
#     version-update PRs have been running; they may stop. Security alerts are
#     not affected by this switch.
#   - Check runs already recorded against a commit stay recorded. A PR that is
#     `UNSTABLE` from an earlier red run stays that way until its branch moves.

set -euo pipefail

OWNER="mkolehmainen"
# The workspace definition is the source of truth for the repository set, so a
# repository added there is picked up here without editing this script.
WORKSPACE_YAML="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)/workspace.yaml"

MODE="status"
DRY_RUN=0
for arg in "$@"; do
    case "$arg" in
        --status)  MODE="status" ;;
        --disable) MODE="disable" ;;
        --enable)  MODE="enable" ;;
        --dry-run) DRY_RUN=1 ;;
        -h|--help) sed -n '3,31p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) echo "unknown argument: $arg (try --help)" >&2; exit 2 ;;
    esac
done

# Repository names from workspace.yaml's `- name:` entries.
repos() {
    if [[ ! -f "$WORKSPACE_YAML" ]]; then
        echo "cannot find workspace.yaml at $WORKSPACE_YAML" >&2
        exit 1
    fi
    sed -n 's/^[[:space:]]*-[[:space:]]*name:[[:space:]]*\(zl-zpr-[a-z0-9-]*\).*/\1/p' "$WORKSPACE_YAML"
}

# Current value of the repository-level Actions switch, or "?" if unreadable.
actions_enabled() {
    gh api "repos/$OWNER/$1/actions/permissions" -q '.enabled' 2>/dev/null || echo "?"
}

# Count of workflows GitHub has registered for the repository -- useful context,
# since a fork can hold workflow files that have never been registered.
workflow_count() {
    gh api "repos/$OWNER/$1/actions/workflows" -q '.total_count' 2>/dev/null || echo "?"
}

names=$(repos)
if [[ -z "$names" ]]; then
    echo "no zl-zpr-* repositories found in $WORKSPACE_YAML" >&2
    exit 1
fi

if [[ "$MODE" == "status" ]]; then
    printf '%-24s %-16s %s\n' "REPOSITORY" "ACTIONS" "WORKFLOWS REGISTERED"
    for r in $names; do
        printf '%-24s %-16s %s\n' "$r" "$(actions_enabled "$r")" "$(workflow_count "$r")"
    done
    echo
    echo "Nothing changed. Re-run with --disable or --enable to act."
    exit 0
fi

target=false
[[ "$MODE" == "enable" ]] && target=true

echo "Setting actions.enabled=$target on $(echo "$names" | wc -w | tr -d ' ') repositories under $OWNER."
[[ "$DRY_RUN" == 1 ]] && echo "(dry run: no requests will be sent)"
echo

failed=0
for r in $names; do
    before=$(actions_enabled "$r")
    if [[ "$before" == "$target" ]]; then
        printf '%-24s already %s\n' "$r" "$target"
        continue
    fi
    if [[ "$DRY_RUN" == 1 ]]; then
        printf '%-24s would change %s -> %s\n' "$r" "$before" "$target"
        continue
    fi
    # `|| true` deliberately omitted per-call: report the failure and keep going,
    # so one repository without admin rights does not hide the rest of the run.
    if gh api -X PUT "repos/$OWNER/$r/actions/permissions" -F "enabled=$target" >/dev/null 2>&1; then
        after=$(actions_enabled "$r")
        if [[ "$after" == "$target" ]]; then
            printf '%-24s %s -> %s\n' "$r" "$before" "$after"
        else
            printf '%-24s FAILED to verify (reads back %s)\n' "$r" "$after"
            failed=$((failed + 1))
        fi
    else
        printf '%-24s FAILED (gh api rejected the write)\n' "$r"
        failed=$((failed + 1))
    fi
done

echo
if [[ "$failed" -gt 0 ]]; then
    echo "$failed repository/repositories did not change. Check that your token has admin"
    echo "on them: gh auth status, and the repo must be yours."
    exit 1
fi
[[ "$DRY_RUN" == 1 ]] && exit 0
echo "Done. Reverse this with: $(basename "${BASH_SOURCE[0]}") --$([[ "$MODE" == "disable" ]] && echo enable || echo disable)"
