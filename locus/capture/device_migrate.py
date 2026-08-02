"""One-off reMarkable reorganisation: plan it, review it, snapshot it, then move.

The device tree grew by accretion and its shape now costs real behaviour: reading has a split
identity (`/reading_list` holds the most-annotated document in the system, and the annotation
sweep watches `/Locus/Reading` instead), daily pages accumulate loose forever, and — the sharp
one — **top-level folder names drive ingest categories** (`capture/remarkable.DEFAULT_FOLDER_
CATEGORY`), so renaming a folder silently changes how the owner's handwriting is filed. The
target tree separates the two things that were tangled: who WRITES a folder, and what ACTIVITY
it belongs to.

    /Daily     Locus writes. An inbox: unmarked pages stay loose, marked ones archive by month.
    /Reading   Both write. Proposed | In-Progress | Finished — one lifecycle for everything read.
    /Notes     He writes, Locus ingests. Topic folders beneath it drive category.
    /Admin     Excluded from everything.

THREE DESIGN DECISIONS, each the answer to a way this could go wrong.

**The plan is item-level, and he edits it before it runs.** Folder rules generate defaults into
`vault/device-migration.toml`; per-item `dest` lines win. This is not ceremony — `/reading_list`
is already not homogeneous. It holds an annotated PDF (a book, which belongs in `/Reading/
In-Progress`) *and* a handwritten notebook of a reading list somebody gave him (a note, which
belongs in `/Notes/brevan_howard`). No folder rule can tell those apart, and a rule that guessed
would file a handwritten note as a book and hand it to the annotation sweep. Anything the rules
cannot decide is marked `review = true` and `apply()` REFUSES while any such line survives.

**Documents move, folders do not.** `rmapi mv` on a folder would move its whole subtree in one
call, which is exactly the operation whose blast radius nobody can review. Every move in the plan
is one document to one destination, so the dry-run is a complete and honest statement of what
will happen. Emptied source folders are left behind for him to delete by hand.

**A snapshot precedes the first move.** These are rmapi mutations against the only copy of
several years of handwriting. `snapshot()` pulls every document to local disk first; `apply()`
will not run without evidence that it happened (`require_snapshot`).

Both runners are injectable, so the whole module is unit-testable with no device and no network —
the plan/apply split means `plan()` genuinely writes nothing, the same shape `structure/propose`
uses for corpus-wide runs.
"""

from __future__ import annotations

import logging
import re
import subprocess
import tomllib
from dataclasses import dataclass, field
from datetime import date
from pathlib import Path
from typing import Callable

from locus.reading.deliver_remarkable import RmapiRunner, _ensure_folder, _subprocess_runner

log = logging.getLogger(__name__)

# `rmapi get` writes into the process's working directory and offers no output-path flag, so the
# snapshot runner takes a cwd. Kept separate from RmapiRunner rather than widening it: every other
# rmapi call in Locus is cwd-independent, and a signature change would ripple through six modules.
SnapshotRunner = Callable[[list[str], Path], "tuple[int, str, str]"]

DAILY_ROOT = "/Daily"
READING_ROOT = "/Reading"
NOTES_ROOT = "/Notes"
ADMIN_ROOT = "/Admin"

# Legacy daily page names: `daily-2026-08-01`, delivered one per day since 2026-07-30.
_DAILY_NAME = re.compile(r"daily-(\d{4})-(\d{2})-\d{2}")


@dataclass(frozen=True)
class DeviceItem:
    """One entry from `rmapi find /`."""

    path: str        # absolute, leading slash
    is_folder: bool

    @property
    def name(self) -> str:
        return self.path.rsplit("/", 1)[-1]


@dataclass
class Move:
    """One document, and where it should end up.

    `dest` is the destination FOLDER, not the destination path — rmapi's `mv <source> <dest>`
    takes a folder and keeps the document's name, and nothing here renames anything. Renaming
    during a migration would break `reading/watch.py`, which matches delivered proposals by
    filename stem.
    """

    src: str
    dest: str
    reason: str
    review: bool = False


@dataclass(frozen=True)
class Rule:
    """`prefix` -> `dest`, preserving any intermediate directories below the prefix.

    So `/projects/oqts/notes` under rule `/projects -> /Notes/projects` lands in
    `/Notes/projects/oqts`, and the oqts grouping he made survives the move.
    """

    prefix: str
    dest: str
    reason: str
    review: bool = False


# Order matters: the first matching prefix wins, so the specific reading folders are listed
# before the general `/Locus` sweep that would otherwise swallow them.
DEFAULT_RULES: tuple[Rule, ...] = (
    Rule("/Locus/Reading", READING_ROOT, "reading tree moves out from under /Locus"),
    # Everything else in /Locus is a document we pushed that is not a daily page (daily pages are
    # handled before the rules — see `_plan_one`). Today that is the pour runbook.
    Rule("/Locus", f"{NOTES_ROOT}/projects", "delivered project doc"),
    Rule(
        "/reading_list",
        f"{READING_ROOT}/In-Progress",
        "MIXED FOLDER: books belong here, but a handwritten notebook belongs in "
        "/Notes/<topic> — set `dest` per item",
        review=True,
    ),
    Rule("/engineering", f"{NOTES_ROOT}/engineering", "topic folder -> /Notes (category: coursework)"),
    Rule("/quantum_ml", f"{NOTES_ROOT}/quantum_ml", "topic folder -> /Notes (category: note)"),
    Rule("/brevan_howard", f"{NOTES_ROOT}/brevan_howard", "topic folder -> /Notes (category: note)"),
    Rule("/careers", f"{NOTES_ROOT}/careers", "topic folder -> /Notes (category: career)"),
    Rule("/rough_notes", f"{NOTES_ROOT}/rough_notes", "topic folder -> /Notes (category: note)"),
    Rule("/projects", f"{NOTES_ROOT}/projects", "topic folder -> /Notes (category: project)"),
    Rule("/admin", ADMIN_ROOT, "paperwork, excluded from ingest"),
)

# Never touched. `trash` is the device's own deleted-items area: moving anything out of it would
# undelete it, and moving anything into it would delete his work.
SKIP_PREFIXES: tuple[str, ...] = ("/trash",)

# Already in the target tree, therefore already home. Checked BEFORE the rules so that a second
# `--plan` run over a migrated (or half-migrated) device produces an empty plan rather than a
# page of "no rule matched" review items — which would make the plan file useless exactly when he
# is re-running it to see what is left. This also covers a document he files himself later.
SETTLED_PREFIXES: tuple[str, ...] = (DAILY_ROOT, READING_ROOT, NOTES_ROOT, ADMIN_ROOT)


def _norm(path: str) -> str:
    return "/" + path.strip("/")


def list_device(runner: RmapiRunner) -> list[DeviceItem]:
    """Every document and folder on the device, from a single `rmapi find /`.

    Raises on failure rather than returning an empty list: an empty listing and a transient rmapi
    error are indistinguishable, and "the device is empty" is never a safe thing to act on — the
    same rule `reading/watch.list_reading_entries` follows for the same reason.
    """
    code, out, err = runner(["find", "/"])
    if code != 0:
        raise RuntimeError(f"rmapi find / failed: {err.strip() or out.strip()}")
    items: list[DeviceItem] = []
    for line in out.splitlines():
        line = line.strip()
        if line.startswith("[f] "):
            items.append(DeviceItem(_norm(line[4:].strip()), is_folder=False))
        elif line.startswith("[d] "):
            items.append(DeviceItem(_norm(line[4:].strip()), is_folder=True))
    if not items:
        raise RuntimeError("rmapi find / returned nothing — refusing to plan against an empty tree")
    return items


def _daily_destination(name: str) -> str | None:
    """`daily-2026-08-01` -> `/Daily/2026-08`, or None if this is not a daily page.

    Every delivered page predates this migration, so all of them are history and archive by month.
    The live inbox behaviour — unmarked pages staying loose at `/Daily` until they have ink on
    them — is the composer's job from here on, not the migration's.
    """
    m = _DAILY_NAME.fullmatch(name)
    return f"{DAILY_ROOT}/{m.group(1)}-{m.group(2)}" if m else None


def _plan_one(item: DeviceItem, rules: tuple[Rule, ...]) -> Move | None:
    path = item.path
    if any(path == p or path.startswith(p + "/") for p in SKIP_PREFIXES + SETTLED_PREFIXES):
        return None

    if path.startswith("/Locus/"):
        daily = _daily_destination(item.name)
        if daily:
            return Move(path, daily, "daily page -> monthly archive")

    for rule in rules:
        if path != rule.prefix and not path.startswith(rule.prefix + "/"):
            continue
        # Directories BELOW the rule's prefix are preserved; the document's own name is not part
        # of the destination folder.
        remainder = path[len(rule.prefix):].strip("/")
        subdirs = remainder.rsplit("/", 1)[0] if "/" in remainder else ""
        dest = f"{rule.dest}/{subdirs}" if subdirs else rule.dest
        return Move(path, dest, rule.reason, review=rule.review)

    # No rule matched. Deliberately NOT given a guessed destination: an unrecognised folder is
    # more likely to be something new he made than something we forgot, and a wrong default here
    # files his writing under the wrong ingest category silently.
    return Move(path, "", "no rule matched — set `dest` yourself", review=True)


def plan(items: list[DeviceItem], *, rules: tuple[Rule, ...] = DEFAULT_RULES) -> list[Move]:
    """Proposed destination for every document. Pure — writes nothing, touches no device."""
    moves: list[Move] = []
    for item in items:
        if item.is_folder:
            continue  # folders are created at the destination, never moved (see module docstring)
        move = _plan_one(item, rules)
        if move is None:
            continue
        if move.dest and move.src.rsplit("/", 1)[0] == move.dest:
            continue  # already where it belongs — a re-run after a partial migration is a no-op
        moves.append(move)
    return sorted(moves, key=lambda m: m.src)


@dataclass
class Validation:
    needs_review: list[Move] = field(default_factory=list)
    collisions: list[tuple[Move, Move]] = field(default_factory=list)

    @property
    def ok(self) -> bool:
        return not self.needs_review and not self.collisions


def validate(moves: list[Move]) -> Validation:
    """Everything that must be resolved before a single `rmapi mv` runs.

    Collisions matter because rmapi refuses a same-named document in one folder (the same
    behaviour that made `rmapi put` fail silently on the 2026-07-30 daily deploy). Catching them
    here turns a half-finished migration into an edit to a text file.
    """
    result = Validation()
    seen: dict[str, Move] = {}
    for move in moves:
        if move.review or not move.dest:
            result.needs_review.append(move)
            continue
        key = f"{move.dest}/{move.src.rsplit('/', 1)[-1]}"
        if key in seen:
            result.collisions.append((seen[key], move))
        else:
            seen[key] = move
    return result


# --- the plan file -------------------------------------------------------------------------

_HEADER = """\
# Locus device migration plan — generated {stamp}
#
# Every line is one document and the folder it will move to. EDIT `dest` freely.
# `review = true` means the rules could not decide; set it to false once you have.
# `locus device-migrate --apply` refuses while any `review = true` line remains.
#
# Delete a block entirely to leave that document exactly where it is.
"""


def _toml_str(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def write_plan(path: Path, moves: list[Move], *, on: date | None = None) -> Path:
    """Render the plan to TOML for him to edit. Overwrites; the plan is regenerable."""
    lines = [_HEADER.format(stamp=(on or date.today()).isoformat())]
    for move in moves:
        lines.append("[[move]]")
        lines.append(f"src    = {_toml_str(move.src)}")
        lines.append(f"dest   = {_toml_str(move.dest)}")
        lines.append(f"reason = {_toml_str(move.reason)}")
        if move.review:
            lines.append("review = true")
        lines.append("")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines), encoding="utf-8")
    return path


def read_plan(path: Path) -> list[Move]:
    """Read back the (possibly hand-edited) plan."""
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    return [
        Move(
            src=_norm(str(row["src"])),
            dest=_norm(str(row["dest"])) if str(row.get("dest", "")).strip() else "",
            reason=str(row.get("reason", "")),
            review=bool(row.get("review", False)),
        )
        for row in data.get("move", [])
    ]


# --- snapshot ------------------------------------------------------------------------------


def _snapshot_runner(binary: str) -> SnapshotRunner:
    def run(args: list[str], cwd: Path) -> tuple[int, str, str]:
        proc = subprocess.run(
            [binary, *args], capture_output=True, text=True, timeout=600, cwd=str(cwd)
        )
        return proc.returncode, proc.stdout, proc.stderr

    return run


@dataclass
class SnapshotResult:
    directory: Path
    fetched: list[str] = field(default_factory=list)
    failed: list[tuple[str, str]] = field(default_factory=list)

    @property
    def complete(self) -> bool:
        return bool(self.fetched) and not self.failed


def snapshot(
    items: list[DeviceItem],
    dest_dir: Path,
    *,
    runner: SnapshotRunner | None = None,
    rmapi_binary: str = "rmapi",
) -> SnapshotResult:
    """Pull every document to local disk before anything is moved.

    Each document is fetched into a local directory mirroring its device path, so the snapshot is
    readable as the tree it came from rather than a flat pile of `.rmdoc` files. A failure is
    recorded, not raised — the caller decides, and `apply(require_snapshot=True)` will not proceed
    on an incomplete one.
    """
    runner = runner or _snapshot_runner(rmapi_binary)
    result = SnapshotResult(directory=dest_dir)
    for item in items:
        if item.is_folder:
            continue
        local_dir = dest_dir / item.path.strip("/").rsplit("/", 1)[0]
        local_dir.mkdir(parents=True, exist_ok=True)
        code, out, err = runner(["get", item.path], local_dir)
        if code != 0:
            result.failed.append((item.path, (err.strip() or out.strip())[:200]))
        else:
            result.fetched.append(item.path)
    return result


# --- apply ---------------------------------------------------------------------------------


@dataclass
class Applied:
    move: Move
    ok: bool
    error: str = ""


def ensure_destinations(runner: RmapiRunner, moves: list[Move]) -> None:
    """`mkdir` every destination folder, one level at a time (rmapi does not do `-p`)."""
    wanted = sorted({m.dest for m in moves if m.dest})
    for dest in wanted:
        parts = [p for p in dest.split("/") if p]
        for i in range(len(parts)):
            _ensure_folder(runner, "/".join(parts[: i + 1]))


def apply(
    moves: list[Move],
    *,
    runner: RmapiRunner | None = None,
    rmapi_binary: str = "rmapi",
    snapshot_result: SnapshotResult | None = None,
    require_snapshot: bool = True,
) -> list[Applied]:
    """Execute the plan. Refuses outright unless it validates and a snapshot exists.

    One failed move does not abort the rest — the same rule the ingest pipeline follows for one
    bad document. A partial migration is recoverable (re-plan and re-run; a document already at
    its destination simply produces no move next time); an aborted one halfway through is not
    obviously either finished or unstarted.
    """
    check = validate(moves)
    if not check.ok:
        raise RuntimeError(
            f"plan is not ready: {len(check.needs_review)} item(s) need review, "
            f"{len(check.collisions)} name collision(s)"
        )
    if require_snapshot and not (snapshot_result and snapshot_result.complete):
        raise RuntimeError(
            "refusing to move anything without a complete local snapshot "
            "(pass require_snapshot=False only if you have a backup by other means)"
        )

    runner = runner or _subprocess_runner(rmapi_binary)
    ensure_destinations(runner, moves)

    results: list[Applied] = []
    for move in moves:
        code, out, err = runner(["mv", move.src, move.dest])
        if code != 0:
            message = (err.strip() or out.strip())[:200]
            log.warning("device-migrate: %s -> %s failed: %s", move.src, move.dest, message)
            results.append(Applied(move, ok=False, error=message))
        else:
            results.append(Applied(move, ok=True))
    return results
