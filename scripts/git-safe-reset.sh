#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: git-safe-reset.sh [--force] <reset-args>

Creates a timestamped backup of the current worktree, validates that the tree
is clean (unless --force), and then runs `git reset` with the provided
arguments. Requires an explicit 'yes' confirmation before performing a
hard reset.
USAGE
}

main() {
  if [[ "${1:-}" == "--help" || $# -eq 0 ]]; then
    usage
    exit 0
  fi

  local force=0
  if [[ "${1:-}" == "--force" ]]; then
    force=1
    shift
  fi

  local root
  root=$(git rev-parse --show-toplevel)

  if [[ $force -eq 0 ]]; then
    local status
    status=$(git status --porcelain)
    if [[ -n "$status" ]]; then
      echo "Refusing to reset: working tree is not clean. Stash or commit changes, or rerun with --force." >&2
      exit 1
    fi
  fi

  mkdir -p "$root/tmp/git-safe-reset"
  local timestamp
  timestamp=$(date +%Y%m%d%H%M%S)
  local archive="$root/tmp/git-safe-reset/worktree-$timestamp.tar.gz"

  echo "Creating backup archive at $archive ..."
  tar -czf "$archive" --exclude='.git' -C "$root" .
  echo "Backup complete." 

  echo "About to run: git reset $*"
  read -r -p "Type 'yes' to continue: " confirmation
  if [[ "$confirmation" != "yes" ]]; then
    echo "Aborted." >&2
    exit 1
  fi

  git reset "$@"
}

main "$@"
