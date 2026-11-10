"""Secrets out of transcripts before anything leaves the laptop (Slice 0.9).

A Claude Code transcript holds whatever passed through the session: a pasted API key, an
`Authorization: Bearer` header in a curl, a private key read from disk, a password typed into a
command. 0.10 copies transcripts to the server, where jobs and the index read them, so every line
is scrubbed first and only the scrubbed copy is ever written off the laptop.

Two kinds of rule:

* **Patterns** for secrets with a recognisable shape: provider key prefixes (Anthropic, OpenAI,
  GitHub, AWS, Slack, Google), bearer tokens, PEM private keys, and a value following a word like
  `password`, `token` or `api_key`.
* **Known secrets**, exact strings, one per line in `~/.config/locus/known-secrets` (mode 0600):
  the kairos, keryx and locus tokens, which are bare hex and have no shape a pattern could find.

What is deliberately NOT a rule: long hex in general. Commit ids and the library's content
hashes are hex too, and redacting them would destroy the text the index is for.

Transcripts are JSON lines, so a secret usually sits inside a JSON string with escaped quotes and
`\\n`. Every replacement is plain `[redacted:<kind>]`, with no quote or backslash, so a scrubbed
line stays valid JSON (a test asserts it).
"""

from __future__ import annotations

import os
import re
from pathlib import Path

KNOWN_SECRETS_PATH = Path(os.environ.get("LOCUS_KNOWN_SECRETS", "~/.config/locus/known-secrets")).expanduser()

# Where a secret may start: not after a word character, or right after a JSON escape (`\n`,
# `\t`), whose letter would otherwise count as one. In a transcript most lines of pasted text
# begin after a `\n`, and a plain `\b` missed every key there (caught by the JSON test, 10 Nov).
_B = r"(?:(?<=\\[ntr])|(?<![A-Za-z0-9_]))"

# (kind, pattern). Order matters only where two could match the same text: the specific
# provider prefixes run before the generic bearer and key=value rules.
_PATTERNS: list[tuple[str, re.Pattern[str]]] = [
    ("private-key", re.compile(
        r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----", re.S)),
    ("anthropic-key", re.compile(r"sk-ant-[A-Za-z0-9_\-]{20,}")),
    ("openai-key", re.compile(_B + r"sk-(?:proj-)?[A-Za-z0-9_\-]{32,}")),
    ("github-token", re.compile(_B + r"(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,})")),
    ("aws-key", re.compile(_B + r"(?:AKIA|ASIA)[0-9A-Z]{16}\b")),
    ("slack-token", re.compile(_B + r"xox[abposr]-[A-Za-z0-9\-]{10,}")),
    ("google-key", re.compile(_B + r"AIza[0-9A-Za-z_\-]{35}\b")),
    ("bearer", re.compile(r"(?i)(" + _B + r"bearer\s+)[A-Za-z0-9._~+/=\-]{16,}")),
    # `password: x`, `"api_key": "x"`, `token=x`, `pw is x`: the word is kept, the value goes.
    # `\\?["']?` allows the escaped quotes a JSON-encoded line carries.
    ("credential", re.compile(
        r"(?i)(" + _B + r"(?:password|passwd|passphrase|pwd|pw|secret|client_secret|token|api[_\-]?key|"
        r"access[_\-]?key|auth[_\-]?token)\b(?:\\?[\"'])?\s*(?:[:=]|\bis\b)\s*(?:\\?[\"'])?)"
        r"([^\s\"'\\,;}{\]\[)(<>]{6,})")),
]

# A credential value that is plainly not a secret: a placeholder, an env reference, a type.
_NOT_A_SECRET = re.compile(r"^(?:\$\{?[A-Z_]+\}?|<[^>]*>|x{4,}|\*{4,}|none|null|true|false|"
                           r"str|string|optional|required|redacted.*|\.\.\.)$", re.I)


def load_known(path: Path = KNOWN_SECRETS_PATH) -> list[str]:
    """Exact secrets to remove, longest first so one inside another cannot leave a tail."""
    try:
        lines = path.read_text().splitlines()
    except OSError:
        return []
    known = {s.strip() for s in lines if len(s.strip()) >= 8 and not s.startswith("#")}
    return sorted(known, key=len, reverse=True)


def scrub(text: str, known: list[str] | None = None) -> tuple[str, dict[str, int]]:
    """The text with every secret replaced, and how many of each kind went."""
    counts: dict[str, int] = {}

    def note(kind: str, n: int = 1) -> None:
        counts[kind] = counts.get(kind, 0) + n

    for s in known or []:
        if s in text:
            note("known", text.count(s))
            text = text.replace(s, "[redacted:known]")
    for kind, pat in _PATTERNS:
        if kind in ("bearer", "credential"):
            def keep_label(m: re.Match[str], kind: str = kind) -> str:
                value = m.group(2) if kind == "credential" else m.group(0)[len(m.group(1)):]
                if kind == "credential" and _NOT_A_SECRET.match(value):
                    return m.group(0)
                note(kind)
                return f"{m.group(1)}[redacted:{kind}]"
            text = pat.sub(keep_label, text)
        else:
            text, n = pat.subn(f"[redacted:{kind}]", text)
            if n:
                note(kind, n)
    return text, counts


def scrub_file(src: Path, dst: Path, known: list[str]) -> dict[str, int]:
    """Scrub one transcript line by line into `dst` (written whole, then renamed into place)."""
    total: dict[str, int] = {}
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp = dst.with_suffix(dst.suffix + ".part")
    with src.open("r", encoding="utf-8", errors="replace") as fin, tmp.open("w", encoding="utf-8") as fout:
        for line in fin:
            out, c = scrub(line, known)
            for k, v in c.items():
                total[k] = total.get(k, 0) + v
            fout.write(out)
    os.chmod(tmp, 0o600)
    tmp.replace(dst)
    return total
