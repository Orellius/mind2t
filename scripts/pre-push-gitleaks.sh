#!/bin/sh
# Pre-push secret scan. Install it with:
#
#     ln -sf ../../scripts/pre-push-gitleaks.sh .git/hooks/pre-push
#
# WHY A PUSH AND NOT A COMMIT. A commit is local and reversible. A push is the
# moment a secret leaves this machine, and on a PUBLIC repository it is the
# moment it is indexed. Scanning here is the last point where deleting the
# commit is still enough.
#
# WHY FULL HISTORY AND NOT THE DIFF. Making a repository public exposes every
# commit ever made, so a credential removed three months ago is still served by
# the GitHub API. A diff-only scan is the guard that looks in the wrong place
# and is therefore never seen to fire.
#
# FAIL CLOSED, DELIBERATELY. If gitleaks is not installed this refuses the push
# rather than waving it through: a scanner that silently skips is indistinguishable
# from one that found nothing, which is the whole failure mode this exists to stop.
#
# The documented escape hatch, for the case where you have read the finding and
# know it is a fixture:
#
#     git push --no-verify
#
# Do not widen a .gitleaks.toml to make a push succeed. Allowlisting is for values
# that authenticate nothing, one path and one rule at a time, with the reason
# written down.
#
# This repository scanned clean over its full history on 2026-08-08 (254 commits).
set -eu

if ! command -v gitleaks >/dev/null 2>&1; then
    echo "pre-push: gitleaks is not installed - refusing to push unscanned." >&2
    echo "          brew install gitleaks    (or: git push --no-verify)" >&2
    exit 1
fi

echo "pre-push: scanning full history for secrets..." >&2
if ! gitleaks detect --no-banner --redact; then
    echo >&2
    echo "pre-push: BLOCKED - gitleaks found something above." >&2
    echo "          Real credential: rotate it first, then remove it from history." >&2
    echo "          Known fixture:   git push --no-verify, and consider a" >&2
    echo "                           .gitleaks.toml allowlist entry naming why." >&2
    exit 1
fi

exit 0
