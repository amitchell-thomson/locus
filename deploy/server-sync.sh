#!/usr/bin/env bash
# Server half of deploy/deploy.sh. Runs in the server checkout AFTER `git pull`, so this file is
# always the version that was just deployed. Argument: the commit the server was on before.
#
# Safe to re-run with no new commits: every step is idempotent, so a deploy with nothing to pull
# still repairs drift between deploy/systemd/ and ~/.config/systemd/user/.
set -euo pipefail

old="${1:?usage: server-sync.sh <previous-commit>}"
new="$(git rev-parse HEAD)"
UNITDIR="$HOME/.config/systemd/user"
SRC=deploy/systemd
export PATH="$HOME/.local/bin:$PATH"

changed_files="$(git diff --name-only "$old" "$new")"

# 0. One-time (2026-10-19): the data directory `vault/` becomes `data/`. The pull has just created
#    `data/` holding only the tracked `.gitkeep`, so the old directory's entries move into it.
#    Taken under the ingest flock so no ingest is writing into the raw store mid-move; the lock
#    file moves with the directory, so an ingest started meanwhile finds it held and skips its
#    turn (`ingest_lock` does not wait). config.toml (gitignored) is rewritten to the new paths, keeping a copy. Migration
#    0037 then rewrites the stored paths (step 2). Remove this block once the server has run it.
if [[ -d vault && ! -L vault && ! -e data/locus.db ]]; then
  echo "== one-time: vault/ -> data/"
  exec 9>>vault/.ingest.lock
  flock -w 900 9 || { echo "deploy: an ingest held the lock for 15 min; not moving" >&2; exit 1; }
  mkdir -p data
  shopt -s dotglob
  mv vault/* data/
  shopt -u dotglob
  rmdir vault
  cp config.toml config.toml.bak-pre-data
  sed -i -E 's#^([a-z_]+ *= *")vault/#\1data/#' config.toml
  grep -n '"data/' config.toml
  flock -u 9
  exec 9>&-
fi

# 1. Dependencies. --all-extras because the server runs every extra (rerank, mathocr, reading,
#    tui): a bare `uv sync` removes the ones it is not told about, torch included.
echo "== uv sync --all-extras"
uv sync --all-extras --quiet

# 1b. The Rust binaries (kairos, ADR-008). Built when Rust sources or the lockfile moved, or a
#     binary is missing; `--locked` so the server runs exactly the lockfile the laptop tested.
#     `cargo install` replaces the binary in ~/.cargo/bin atomically; the running daemon keeps
#     its old inode until step 5 restarts it.
kairos_built=0
if grep -qE '^(kairos/|crates/|Cargo\.(toml|lock)$)' <<<"$changed_files" \
   || [[ ! -x "$HOME/.cargo/bin/kairosd" || ! -x "$HOME/.cargo/bin/kairos" ]]; then
  echo "== cargo install kairosd, kairos"
  cargo install --quiet --locked --path kairos/crates/kairosd
  cargo install --quiet --locked --path kairos/crates/kairos-cli
  kairos_built=1
fi

# 1c. One-time (2026-10-24): kairosd's unit was `kairosd.service`, installed by hand from the old
#     kairos repository. It becomes locus-kairosd.service (step 3); stop the old one first so the
#     two never hold the port together. Remove this block once the server has run it.
if [[ -e "$UNITDIR/kairosd.service" ]]; then
  echo "== one-time: kairosd.service -> locus-kairosd.service"
  systemctl --user disable --now kairosd.service || true
  mv "$UNITDIR/kairosd.service" "$UNITDIR/kairosd.service.bak-2026-10-24"
  systemctl --user daemon-reload
  kairos_built=1
fi

# 2. Schema. Backup first, then migrate, only when a migration arrived; otherwise the schema
#    would move whenever the next ingest or `locus mcp` happened to call migrate().
if grep -q '^locus/db/migrations/versions/' <<<"$changed_files"; then
  echo "== new migration: backup, then migrate"
  uv run locus backup
  uv run locus migrate
fi

# 3. Unit files: the repo is the source. Copy what differs, remove locus units the repo dropped.
mkdir -p "$UNITDIR"
changed_units=()
for f in "$SRC"/locus-*; do
  name="$(basename "$f")"
  if ! cmp -s "$f" "$UNITDIR/$name"; then
    cp "$f" "$UNITDIR/$name"
    changed_units+=("$name")
  fi
done
for f in "$UNITDIR"/locus-*; do
  [[ -e "$f" ]] || continue
  name="$(basename "$f")"
  if [[ ! -e "$SRC/$name" ]]; then
    echo "== removing $name (no longer in $SRC)"
    systemctl --user disable --now "$name" 2>/dev/null || true
    rm -f "$f"
    changed_units+=("$name")
  fi
done
if ((${#changed_units[@]})); then
  echo "== unit files changed: ${changed_units[*]}"
  systemctl --user daemon-reload
fi

# 4. Enablement: deploy/systemd/enabled is the list of units that are ON.
mapfile -t wanted < <(sed -e 's/#.*//' -e 's/[[:space:]]//g' "$SRC/enabled" | grep -v '^$')
for unit in "${wanted[@]}"; do
  systemctl --user enable --now "$unit" 2>/dev/null \
    || { echo "deploy: could not enable $unit" >&2; exit 1; }
done
while read -r unit _; do
  [[ -n "$unit" ]] || continue
  if ! printf '%s\n' "${wanted[@]}" | grep -qx "$unit"; then
    echo "== disabling $unit (not in $SRC/enabled)"
    systemctl --user disable --now "$unit"
  fi
done < <(systemctl --user list-unit-files 'locus-*' --state=enabled --no-legend)

# 5. Restarts. A timer-fired oneshot runs `uv run locus ...` from this checkout at each firing, so
#    it picks up new code by itself and is never restarted. What does need it: a timer whose own
#    file changed, and every long-running service (listed as a .service in `enabled`) when the
#    code moved or its unit changed, since it runs the code it started with.
restarted=()
for unit in "${wanted[@]}"; do
  unit_changed=0
  for c in "${changed_units[@]+"${changed_units[@]}"}"; do
    [[ "$c" == "$unit" ]] && unit_changed=1
  done
  # kairosd is compiled, so a code move matters to it only when its build ran (step 1b).
  moved=0
  if [[ "$unit" == locus-kairosd.service ]]; then
    ((kairos_built)) && moved=1
  elif [[ "$old" != "$new" ]]; then
    moved=1
  fi
  if [[ "$unit" == *.timer && $unit_changed == 1 ]] \
     || [[ "$unit" == *.service && ( $unit_changed == 1 || $moved == 1 ) ]]; then
    systemctl --user restart "$unit"
    restarted+=("$unit")
  fi
done

# 6. The check: what the server now runs, and the state of every unit that should be on.
echo
echo "== server at"
git log -1 --format='%h %s (%cr)'
echo "== restarted: ${restarted[*]:-none}"
echo "== units"
status=0
for unit in "${wanted[@]}"; do
  state="$(systemctl --user is-active "$unit" || true)"
  printf '  %-36s %s\n' "$unit" "$state"
  [[ "$state" == active ]] || status=1
done
for unit in "${restarted[@]+"${restarted[@]}"}"; do
  echo
  systemctl --user status "$unit" --no-pager --lines=0 | head -4
done
if command -v kairos >/dev/null; then
  echo
  echo "== kairos doctor"
  kairos doctor || status=1
fi
exit $status
