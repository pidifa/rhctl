#!/bin/bash
# ************************************************************************************
# Copyright (C) 2022-2026 rhctl Contributors
#
# SPDX-License-Identifier: Apache-2.0
# ************************************************************************************
# Publish a Git release and automatically merge the source branch into the target branch.
#
# The script pushes the source branch to the remote, creates the versioned release branch
# and tag, publishes the GitHub release using the specified release notes, then creates,
# approves, and merges a pull request from the source branch into the target branch.
#
# If exists, skip — safe to re-run.
#
# Usage:
#   ./release.sh \
#     --repo /path/to/repository \
#     --version v1.0.1 \
#     --from-branch dev \
#     --release-notes ./release-notes/v1.0.1.md
#
# Required Parameters:
#   --repo <directory>
#       Git repository root directory.
#         Example directory values: `/home/user/project`, `/mnt/c/Users/user/project`
#
#   --version <version>
#       Release version.
#         Example version values: `v1.0.1`, `v2.0.0`
#
#   --from-branch <branch>
#       Source branch to publish and merge into the target branch.
#         Example branch values: `dev`, `release/v1.0.0`
#
#   --release-notes <file>
#       Markdown file containing the GitHub release notes.
#         Example file values: `./release-notes/v1.0.1.md`, `/tmp/v1.0.1.md`
#
# Optional Parameters:
#   --to-branch <branch>
#       Target branch for the post-release pull request (default: `main`).
#         Example branch values: `main`, `master`
#
#   --remote <name>
#       Git remote used for the release (default: `origin`).
#         Example name values: `origin`, `upstream`
#
# Override Parameters:
#   RHCTL_RELEASE_REMOTE=<name>
#       Same as `--remote`.
#
# Since : 1.0.0
# Date  : Oct 8, 2026
# ************************************************************************************

set -euo pipefail

# ========================================================================= Parameter

REPO_DIR=""
VERSION=""
FROM_BRANCH=""
RELEASE_NOTES=""

TO_BRANCH="main"
REMOTE="${RHCTL_RELEASE_REMOTE:-origin}"

usage() {
    sed -n '1,75p' "$0"
    exit 1
}

error() {
    echo "Error: $*" >&2
    exit 1
}

# ========================================================================= Argument parsing

while [[ $# -gt 0 ]]; do
    case "$1" in
        --repo)
            [[ $# -ge 2 ]] || error "Missing value for --repo."
            REPO_DIR="$2"
            shift 2
            ;;
        --version)
            [[ $# -ge 2 ]] || error "Missing value for --version."
            VERSION="$2"
            shift 2
            ;;
        --from-branch)
            [[ $# -ge 2 ]] || error "Missing value for --from-branch."
            FROM_BRANCH="$2"
            shift 2
            ;;
        --release-notes)
            [[ $# -ge 2 ]] || error "Missing value for --release-notes."
            RELEASE_NOTES="$2"
            shift 2
            ;;
        --to-branch)
            [[ $# -ge 2 ]] || error "Missing value for --to-branch."
            TO_BRANCH="$2"
            shift 2
            ;;
        --remote)
            [[ $# -ge 2 ]] || error "Missing value for --remote."
            REMOTE="$2"
            shift 2
            ;;
        -h|--help)
            usage
            ;;
        *)
            error "Unknown parameter: $1"
            ;;
    esac
done

# ========================================================================= Parameter validation

[[ -n "$REPO_DIR" ]] \
    || error "--repo is required."

[[ -n "$VERSION" ]] \
    || error "--version is required."

[[ -n "$FROM_BRANCH" ]] \
    || error "--from-branch is required."

[[ -n "$RELEASE_NOTES" ]] \
    || error "--release-notes is required."

[[ "$VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] \
    || error "Invalid version '$VERSION'. Expected format: v1.0.1."

[[ -n "$TO_BRANCH" ]] \
    || error "--to-branch cannot be empty."

[[ -n "$REMOTE" ]] \
    || error "--remote cannot be empty."

# ========================================================================= Repository discovery

[[ -d "$REPO_DIR" ]] \
    || error "Repository directory does not exist: ${REPO_DIR}"

REPO_DIR="$(cd "$REPO_DIR" && pwd)"

git -C "$REPO_DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1 \
    || error "Not a Git repository: ${REPO_DIR}"

REPO_ROOT="$(git -C "$REPO_DIR" rev-parse --show-toplevel)"

cd "$REPO_ROOT"

command -v gh >/dev/null 2>&1 \
    || error "GitHub CLI 'gh' is not installed."

git remote get-url "$REMOTE" >/dev/null 2>&1 \
    || error "Git remote '$REMOTE' does not exist."

# ========================================================================= Validation

[[ -f "$RELEASE_NOTES" ]] \
    || error "Release notes file does not exist: ${RELEASE_NOTES}"

[[ -s "$RELEASE_NOTES" ]] \
    || error "Release notes file is empty: ${RELEASE_NOTES}"

git show-ref --verify --quiet "refs/heads/${FROM_BRANCH}" \
    || error "Source branch does not exist locally: ${FROM_BRANCH}"

gh auth status >/dev/null 2>&1 \
    || error "GitHub CLI is not authenticated."

# ========================================================================= Push from-branch to remote

echo "==> Push ${FROM_BRANCH} -> ${REMOTE}/${FROM_BRANCH}"

git push "$REMOTE" "refs/heads/${FROM_BRANCH}:refs/heads/${FROM_BRANCH}"

RELEASE_BRANCH="release/${VERSION#v}"

# ========================================================================= Release branch

echo "==> Push ${FROM_BRANCH} -> ${REMOTE}/${RELEASE_BRANCH}"

git push "$REMOTE" "refs/heads/${FROM_BRANCH}:refs/heads/${RELEASE_BRANCH}"

# ========================================================================= Release tag

echo "==> Create tag ${VERSION} (force)"

git tag --force "$VERSION"

echo "==> Push tag ${VERSION} (force)"

git push --force "$REMOTE" "$VERSION"

# ========================================================================= GitHub release

if gh release view "$VERSION" >/dev/null 2>&1; then
    echo "==> Delete existing GitHub release ${VERSION}"

    gh release delete "$VERSION" --yes
fi

echo "==> Create GitHub release ${VERSION}"

gh release create "$VERSION" \
    --title "$VERSION" \
    --notes-file "$RELEASE_NOTES"

# ========================================================================= Pull request

PR_NUMBER=""

if [[ "$FROM_BRANCH" == "$TO_BRANCH" ]]; then
    echo "==> Source and target branches are identical; skipping pull request."
else
    EXISTING_PR="$(
        gh pr list \
            --base "$TO_BRANCH" \
            --head "$FROM_BRANCH" \
            --state open \
            --json number \
            --jq '.[0].number'
    )"

    if [[ -n "$EXISTING_PR" ]]; then
        PR_NUMBER="$EXISTING_PR"
        echo "==> Pull request already exists: #${PR_NUMBER}"
    else
        echo "==> Create pull request: ${FROM_BRANCH} -> ${TO_BRANCH}"

        PR_URL="$(
            gh pr create \
                --base "$TO_BRANCH" \
                --head "$FROM_BRANCH" \
                --title "Merge ${FROM_BRANCH} into ${TO_BRANCH} after ${VERSION}" \
                --body "Merge ${FROM_BRANCH} into ${TO_BRANCH} after releasing ${VERSION}."
        )"

        PR_NUMBER="${PR_URL##*/}"

        [[ "$PR_NUMBER" =~ ^[0-9]+$ ]] \
            || error "Failed to determine pull request number from: ${PR_URL}"

        [[ -n "$PR_NUMBER" ]] \
            || error "Failed to create pull request."
    fi

# ========================================================================= Pull request approval

    PR_REVIEW_STATE="$(
        gh pr view "$PR_NUMBER" \
            --json reviews \
            --jq '[.reviews[] | select(.author.login == "'"$(gh api user --jq '.login')"'" and .state == "APPROVED")] | length'
    )"

    if [[ "$PR_REVIEW_STATE" -gt 0 ]]; then
        echo "==> Pull request already approved: #${PR_NUMBER}"
    else
        echo "==> Approve pull request: #${PR_NUMBER}"

        gh pr review "$PR_NUMBER" \
            --approve
    fi

# ========================================================================= Pull request merge

    PR_STATE="$(
        gh pr view "$PR_NUMBER" \
            --json state \
            --jq '.state'
    )"

    if [[ "$PR_STATE" == "MERGED" ]]; then
        echo "==> Pull request already merged: #${PR_NUMBER}"
    else
        echo "==> Merge pull request: #${PR_NUMBER}"

        gh pr merge "$PR_NUMBER" \
            --merge \
            --delete-branch=false
    fi
fi

# ========================================================================= Final output

echo
echo "Release completed:"
echo "  Repository    : ${REPO_ROOT}"
echo "  Version       : ${VERSION}"
echo "  From branch   : ${FROM_BRANCH}"
echo "  Release branch: ${RELEASE_BRANCH}"
echo "  To branch     : ${TO_BRANCH}"
echo "  Release notes : ${RELEASE_NOTES}"
echo "  Remote        : ${REMOTE}"

if [[ -n "$PR_NUMBER" ]]; then
    echo "  Pull request  : #${PR_NUMBER}"
fi