#!/usr/bin/env bash
# Deploy: laptop -> GitHub -> server. Run from the laptop checkout, on `main`.
#
# The one way code reaches the server (Slice 0 step 0.1, plan B.6). The server only ever runs
# `main` and only ever fast-forwards, so what runs there is always a commit that is on GitHub.
#
#   1. push `main` to origin
#   2. on the server: refuse if the checkout has uncommitted changes (prints them), else
#      `git pull --ff-only`
#   3. hand over to deploy/server-sync.sh FROM THE CODE JUST PULLED, which syncs dependencies,
#      migrates (after a backup) when a migration arrived, syncs the systemd units and restarts
#      what changed. Keeping that half in its own file means it is versioned with the units it
#      installs, rather than frozen at whatever this laptop script said.
#
# Live `locus mcp` processes are NOT touched: each is one chat session's stdio child and nothing
# respawns it (CLAUDE.md §13, 2026-09-14). The STALE banner tells a session it runs old code.
#
# Env: LOCUS_HOST (default compute-node), LOCUS_DIR (default /home/alec/server-projects/locus).
set -euo pipefail

HOST="${LOCUS_HOST:-compute-node}"
DIR="${LOCUS_DIR:-/home/alec/server-projects/locus}"

cd "$(git rev-parse --show-toplevel)"

branch="$(git symbolic-ref --short HEAD)"
if [[ "$branch" != main ]]; then
  echo "deploy: on '$branch', not main. The server runs main only; merge first." >&2
  exit 1
fi
if [[ -n "$(git status --porcelain)" ]]; then
  # Only commits are deployed, so local edits are not lost, but they are not going either.
  echo "deploy: note: uncommitted laptop changes are NOT deployed:" >&2
  git status --short >&2
fi

echo "== push main ($(git log -1 --format='%h %s'))"
git push origin main

echo "== server $HOST:$DIR"
# shellcheck disable=SC2087  # $DIR expands locally on purpose; everything else is remote.
ssh "$HOST" bash -s <<EOF
set -euo pipefail
cd "$DIR"
if [[ -n "\$(git status --porcelain)" ]]; then
  echo "deploy: REFUSED. The server checkout has uncommitted changes:" >&2
  git status --short >&2
  echo "Commit them (and push) or discard them on the server, then deploy again." >&2
  exit 1
fi
if [[ "\$(git symbolic-ref --short HEAD)" != main ]]; then
  echo "deploy: REFUSED. The server checkout is on '\$(git symbolic-ref --short HEAD)', not main." >&2
  exit 1
fi
old="\$(git rev-parse HEAD)"
git pull --ff-only origin main
exec deploy/server-sync.sh "\$old"
EOF
