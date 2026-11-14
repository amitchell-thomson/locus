"""Canvas, read only (spec P, the Canvas pull; P-D1).

The token lives on the server only, in `~/.config/locus/canvas.env` (`CANVAS_TOKEN`,
`CANVAS_URL`), mode 600. A student's token can do whatever the student can, which includes
submitting work, so this client has one verb: GET. There is no other method on it, and
`_request` refuses anything else, so nothing in locus can write to Canvas by accident.

What P1 found on the real account (14 Nov 2026):
- `/users/self` and `/courses` work: 33 courses over three years (Prelims P1-P5 in 2024-25,
  Part A A1-A5 in 2025-26, the B papers in 2026-27), plus the department's MEng and exam pages.
- A course's Files listing is **403** for students; its Modules are 200, and a file reached
  through a module item (`/courses/:id/files/:file_id`) is readable and downloads. So the pull
  (P2) walks modules, never the Files tab.

Courses map to the registry by a `canvas = <id>` line on each course entry (P1). The match is
proposed from the paper code ("B18", "A1", "P3") and shown as a diff; nothing is written to the
registry until he approves it (`locus canvas courses --apply`).
"""

from __future__ import annotations

import json
import os
import re
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path

ENV_FILE = Path.home() / ".config" / "locus" / "canvas.env"
DEFAULT_URL = "https://canvas.ox.ac.uk"


class CanvasError(RuntimeError):
    pass


def _read_env(path: Path = ENV_FILE) -> dict[str, str]:
    out: dict[str, str] = {}
    if path.exists():
        for line in path.read_text().splitlines():
            k, sep, v = line.strip().partition("=")
            if sep and not k.startswith("#"):
                out[k.strip()] = v.strip()
    return out


class Client:
    """Canvas over its REST API, GET only. Pages are followed through the Link header."""

    def __init__(self, token: str | None = None, base: str | None = None, opener=None):
        env = _read_env()
        self.token = token or os.environ.get("CANVAS_TOKEN") or env.get("CANVAS_TOKEN")
        self.base = (base or os.environ.get("CANVAS_URL") or env.get("CANVAS_URL") or DEFAULT_URL).rstrip("/")
        if not self.token:
            raise CanvasError(f"no Canvas token: CANVAS_TOKEN in {ENV_FILE} (the server only)")
        self._open = opener or urllib.request.urlopen

    def _request(self, method: str, url: str):
        if method != "GET":
            raise CanvasError(f"refusing {method}: the Canvas client is read only")
        req = urllib.request.Request(url, method="GET", headers={"Authorization": f"Bearer {self.token}"})
        try:
            return self._open(req, timeout=60)
        except urllib.error.HTTPError as e:
            raise CanvasError(f"GET {url.split('?')[0]}: HTTP {e.code}") from e
        except urllib.error.URLError as e:
            raise CanvasError(f"GET {url.split('?')[0]}: {e.reason}") from e

    def get(self, path: str, params: list[tuple[str, str]] | None = None) -> list | dict:
        """One resource, or every page of a list."""
        q = list(params or [])
        if not any(k == "per_page" for k, _ in q):
            q.append(("per_page", "100"))
        url = f"{self.base}/api/v1/{path.lstrip('/')}?{urllib.parse.urlencode(q)}"
        out: list = []
        while url:
            with self._request("GET", url) as r:
                body = json.loads(r.read().decode())
                link = r.headers.get("Link", "") or ""
            if not isinstance(body, list):
                return body
            out.extend(body)
            m = re.search(r'<([^>]+)>;\s*rel="next"', link)
            url = m.group(1) if m else None
        return out


@dataclass(frozen=True)
class Course:
    id: int
    name: str
    term: str

    @property
    def code(self) -> str | None:
        """The paper code: "B18", "A1", "P3"; None for the department's own pages."""
        m = re.match(r"([ABP])(\d{1,2})\b\.?", self.name)
        return f"{m.group(1)}{m.group(2)}" if m else None


def courses(client: Client) -> list[Course]:
    """Every course he is or was enrolled on, newest year first."""
    raw = client.get("courses", [("include[]", "term"), ("state[]", "available"), ("state[]", "completed")])
    out = [Course(int(c["id"]), name(c.get("name") or ""), (c.get("term") or {}).get("name") or "") for c in raw]
    return sorted(out, key=lambda c: (c.term, -(int(c.code[1:]) if c.code else 99)), reverse=True)


# ── the registry mapping (P1) ──────────────────────────────────────────────────────────────────

def _code_of(name: str) -> str | None:
    m = re.match(r"([ABP])(\d{1,2})\b", name)
    return f"{m.group(1)}{m.group(2)}" if m else None


def _folder(name: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")


def name(raw: str) -> str:
    """A Canvas title as the system uses it: Canvas pads some with spaces ("Signal Analysis ")."""
    return " ".join(raw.split())


@dataclass
class Mapping:
    mapped: list[tuple[str, Course]]          # registry id -> course, already in the registry
    new: list[Course]                         # a past paper with no registry entry yet
    renamed: list[tuple[str, str, Course]]    # registry id, its name, the Canvas name
    unmatched: list[Course]                   # department pages and courses he does not take


def plan(entries: dict[str, dict], cs: list[Course]) -> Mapping:
    """Match registry courses to Canvas courses by paper code.

    This year's B papers he is enrolled on but not taking (Canvas enrols every third-year on all
    of them) are left unmatched. Past years' papers (Prelims, Part A) were all compulsory, so each
    becomes a new, inactive registry entry: P-D9 gives Years 1 and 2 the same syllabus structure.
    """
    by_code: dict[str, str] = {}
    for rid, e in entries.items():
        if e.get("kind") == "course" and (c := _code_of(e.get("name", ""))):
            by_code[c] = rid
    m = Mapping([], [], [], [])
    for c in cs:
        rid = by_code.get(c.code or "")
        if rid:
            m.mapped.append((rid, c))
            if entries[rid]["name"] != c.name:
                m.renamed.append((rid, entries[rid]["name"], c))
        elif c.code and c.code[0] in "AP":
            m.new.append(c)
        else:
            m.unmatched.append(c)
    return m


def apply_to_registry(text: str, m: Mapping) -> str:
    """The registry's text with `canvas = <id>` and Canvas's own name on each mapped course
    ("the canvas titles and naming of courses/submodules in my system should all be identical",
    14 Nov; the id and folder are the shorthand), and the new entries
    appended after the last course. Idempotent: an entry that already has the line keeps it."""
    lines = text.splitlines(keepends=True)
    ids = {rid: c.id for rid, c in m.mapped}
    names = {rid: c.name for rid, c in m.mapped}
    out: list[str] = []
    current: str | None = None
    for line in lines:
        s = line.strip()
        if s == "[[projects]]":
            current = None
        if mm := re.match(r'id\s*=\s*"([^"]+)"', s):
            current = mm.group(1)
        if current in ids and re.match(r"canvas\s*=", s):
            continue
        if current in names and re.match(r"name\s*=", s):
            line = f"name = {json.dumps(names[current])}\n"   # his rule (14 Nov): names exactly as on Canvas
        out.append(line)
        if current in ids and re.match(r'kind\s*=\s*"course"', s):
            out.append(f"canvas = {ids[current]}\n")
    text = "".join(out)
    if m.new:
        block = "".join(
            f'\n[[projects]]\nid = "{c.code.lower()}"\nname = "{c.name}"\nfolder = "{_folder(c.name)}"\n'
            f'area = "Studies"\nkind = "course"\ncanvas = {c.id}\nactive = false\n'
            for c in sorted(m.new, key=lambda c: (c.code[0] != "P", int(c.code[1:])))
        )
        anchor = text.find("# ── application")
        header = "# Past years' papers (Prelims 2024-25, Part A 2025-26): inactive, kept for their syllabi (P-D9).\n"
        text = text[:anchor] + header + block + "\n" + text[anchor:] if anchor >= 0 \
            else text + "\n" + header + block
    return text


def fetch_courses() -> list[Course]:
    """The course list: directly where the token is (the server), else from the server over
    ssh, so the registry can be edited in the laptop's checkout."""
    try:
        return courses(Client())
    except CanvasError as e:
        if "no Canvas token" not in str(e):
            raise
    import subprocess

    host = os.environ.get("LOCUS_HOST", "compute-node")
    d = os.environ.get("LOCUS_DIR", "/home/alec/server-projects/locus")
    uv = os.environ.get("LOCUS_SERVER_UV", "~/.local/bin/uv")  # a non-login ssh shell has no PATH to uv
    cmd = ["ssh", "-o", "ConnectTimeout=10", "-o", "BatchMode=yes", host,
           f"cd {d} && {uv} run --quiet locus canvas courses --json"]
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
    if r.returncode != 0:
        raise CanvasError(f"the server could not list Canvas courses: {(r.stderr or r.stdout).strip()[:300]}")
    return [Course(**c) for c in json.loads(r.stdout)]


def modules(client: Client, course_id: int, *, items: bool = False) -> list[dict]:
    """A course's modules in order (Modules are readable to students; the Files tab is not)."""
    params = [("include[]", "items")] if items else []
    return client.get(f"courses/{course_id}/modules", params)


def module_names() -> dict[str, list[str]]:
    """Registry id -> its Canvas module names, for every course with a `canvas` id. On the
    server directly; elsewhere from the server over ssh (the token lives there only)."""
    try:
        client = Client()
    except CanvasError as e:
        if "no Canvas token" not in str(e):
            raise
        import subprocess

        host = os.environ.get("LOCUS_HOST", "compute-node")
        d = os.environ.get("LOCUS_DIR", "/home/alec/server-projects/locus")
        uv = os.environ.get("LOCUS_SERVER_UV", "~/.local/bin/uv")
        r = subprocess.run(["ssh", "-o", "ConnectTimeout=10", "-o", "BatchMode=yes", host,
                            f"cd {d} && {uv} run --quiet locus canvas modules --json"],
                           capture_output=True, text=True, timeout=300)
        if r.returncode != 0:
            raise CanvasError(f"the server could not list Canvas modules: {(r.stderr or r.stdout).strip()[:300]}")
        return json.loads(r.stdout)
    from locus.heuris import registry

    return {rid: [name(m["name"]) for m in modules(client, int(e["canvas"]))]
            for rid, e in registry.entries().items() if e.get("canvas")}
