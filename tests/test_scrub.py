"""Slice 0.9: no planted secret survives the scrubber, ordinary text is untouched, and a scrubbed
transcript line is still valid JSON."""

import json

from locus.scrub import scrub

KNOWN = ["4ff3" + "a1" * 30]  # a bare-hex token, as kairos's is

PLANTED = {
    "anthropic-key": "export ANTHROPIC_API_KEY=sk-ant-api03-" + "Ab1_" * 20,
    "openai-key": "key sk-proj-" + "x9Y" * 15,
    "github-token": "git clone https://ghp_" + "a" * 36 + "@github.com/x/y",
    "aws-key": "AKIAABCDEFGHIJKLMNOP is the id",
    "slack-token": "xoxb-1234567890-abcdefghij",
    "google-key": "AIza" + "B" * 35,
    "bearer": 'curl -H "Authorization: Bearer abc.DEF-ghi_jkl012345678" http://x',
    "private-key": "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk\n-----END OPENSSH PRIVATE KEY-----",
    "credential": "the password: hunter2hunter2 and api_key = q8w7e6r5t4",
    "known": "token in the header was " + KNOWN[0] + " yes",
}

ORDINARY = [
    "commit 9fcda5b3c1d2e4f5a6b7c8d9e0f1a2b3c4d5e6f7 moved the registry",
    "content_hash 3f2a" + "0" * 60,
    "set a password field on the form; the token budget is 4,000",
    "password: ${DB_PASSWORD} comes from the environment",
    "def check_token(token: str) -> bool:",
    "Bearer tokens are sent in the Authorization header.",
]


def test_planted_secrets_do_not_survive():
    for kind, text in PLANTED.items():
        out, counts = scrub(text, KNOWN)
        print(f"{kind:14} BEFORE {text!r}\n{'':14} AFTER  {out!r}")
        assert counts, kind
        for secret_bit in ("hunter2", "q8w7e6", "Ab1_Ab1_", "x9Yx9Y", "aaaaaaaa", "ABCDEFGHIJKLMNOP",
                           "abcdefghij", "BBBBBBBB", "ghi_jkl", "b3BlbnNzaC1rZXk", "a1a1a1a1"):
            assert secret_bit not in out, (kind, out)


def test_ordinary_text_is_unchanged():
    for text in ORDINARY:
        out, counts = scrub(text, KNOWN)
        assert out == text and not counts, (text, out)


def test_a_scrubbed_transcript_line_is_still_json():
    line = json.dumps({"type": "user", "message": {"content": "\n".join(PLANTED.values())}})
    out, counts = scrub(line, KNOWN)
    v = json.loads(out)
    assert "[redacted:" in v["message"]["content"]
    assert len(counts) >= 9
    for secret_bit in ("hunter2", "q8w7e6", "Ab1_Ab1_", "x9Yx9Y", "aaaaaaaa", "ABCDEFGHIJKLMNOP",
                       "abcdefghij", "BBBBBBBB", "ghi_jkl", "b3BlbnNzaC1rZXk", "a1a1a1a1"):
        assert secret_bit not in out, secret_bit
