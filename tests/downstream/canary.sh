#!/usr/bin/env bash
# Downstream canary: run the LimniFS suite against limnifs@main (not
# the pin), and when it passes AND main has moved past the pin, open a
# PR that bumps the pin. When it fails, exit red with the ref — that
# means either we broke something their newest tests catch (before
# anyone bumps the pin), or downstream changed shape; both need eyes.
#
# This closes the staleness loop of a pinned-only gate: LimniFS can
# land a regression test for a fresh omnizip issue at any time, and
# the pin only sees it when a human bumps it. The canary sees it
# within a day and turns itself into the bump.
#
# Usage: tests/downstream/canary.sh   (from the repo root; needs git
# push + gh permissions when opening the bump PR)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PIN="$(cat "$ROOT/tests/downstream/limnifs-ref.txt")"
HEAD="$(git ls-remote https://github.com/limnifs/limnifs.git refs/heads/main | cut -f1)"

echo "==> pin:  $PIN"
echo "==> main: $HEAD"
# CANARY_DRY_RUN=1: report + run the suite, but never push/PR.
DRY="${CANARY_DRY_RUN:-0}"

if [ "$PIN" = "$HEAD" ]; then
    echo "==> pin is current; nothing to do"
    exit 0
fi

echo "==> limnifs main has moved; running the suite at main"
if ! LIMNIFS_REF="$HEAD" "$ROOT/tests/downstream/run.sh"; then
    echo ""
    echo "DOWNSTREAM CANARY RED at limnifs@$HEAD"
    echo "The suite at limnifs main fails against this workspace."
    echo "Either an omnizip change broke something their newest tests"
    echo "catch, or downstream changed shape. Investigate before the"
    echo "next release; do NOT bump the pin past a red canary."
    exit 1
fi

echo "==> suite green at main; proposing pin bump"
if [ "$DRY" = "1" ]; then
    echo "==> dry run: skipping branch/PR"
    exit 0
fi
BRANCH="pin/limnifs-${HEAD:0:10}"
if git ls-remote --quiet --exit-code origin "refs/heads/$BRANCH" >/dev/null; then
    echo "==> bump branch $BRANCH already exists; nothing to do"
    exit 0
fi
cd "$ROOT"
git checkout --quiet -b "$BRANCH" 2>/dev/null || git checkout --quiet "$BRANCH"
printf '%s\n' "$HEAD" > tests/downstream/limnifs-ref.txt
git add tests/downstream/limnifs-ref.txt
git -c user.name="omnizip-ci" -c user.email="ci@omnizip.invalid" \
    commit --quiet -m "ci(downstream): bump limnifs pin to ${HEAD:0:10}

Canary run green: the full LimniFS suite passes at limnifs@$HEAD
against the current workspace. Auto-proposed by
tests/downstream/canary.sh."
git push --quiet origin "$BRANCH"
printf '%s\n' \
    "## What" \
    "" \
    "Downstream canary: the LimniFS suite passes at \`limnifs@$HEAD\` against the current workspace, and the pin trails main. This bumps the pin so the per-PR gate runs their newest tests (including any new omnizip regression coverage)." \
    "" \
    "Auto-proposed by \`tests/downstream/canary.sh\`." > /tmp/pin-bump-body.md
gh pr create --title "ci(downstream): bump limnifs pin to ${HEAD:0:10}" \
    --body-file /tmp/pin-bump-body.md --base main --head "$BRANCH"
echo "==> pin-bump PR opened for ${HEAD:0:10}"
