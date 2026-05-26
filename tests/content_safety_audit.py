#!/usr/bin/env python3
"""Public-repo content-safety audit for C0PL4ND.

Scans the entire product tree for content that must NOT ship in a public
repository: absolute machine paths, internal agent-system references, secrets,
or plan-tracking tokens. Exit 0 = safe to publish; exit 1 = leakage found.

This is the publish gate from the product plan. It defines, by construction,
exactly what is allowed to ship publicly.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

# The product root is the parent of this tests/ directory.
ROOT = Path(__file__).resolve().parent.parent

# Directories that never ship / are build output — skip them.
SKIP_DIRS = {".git", "target", "node_modules", "dist", ".github/cache"}

# Files this audit may itself contain the patterns in (it documents them).
SELF = Path(__file__).resolve()

# Forbidden patterns (regex, reason). Public repos must contain none of these.
FORBIDDEN = [
    (re.compile(r"C:\\Users\\", re.IGNORECASE), "absolute Windows user path"),
    (re.compile(r"/home/[a-z0-9._-]+/", re.IGNORECASE), "absolute Linux home path"),
    (re.compile(r"/Users/[a-z0-9._-]+/", re.IGNORECASE), "absolute macOS home path"),
    (re.compile(r"\.s4f3\b"), "internal agent-system directory reference"),
    (re.compile(r"\.claude\b"), "internal agent-system directory reference"),
    (re.compile(r"\bS4F3\b"), "internal agent-system brand token"),
    (re.compile(r"\bplan-\d{2,4}\b"), "internal plan-tracking token"),
    (re.compile(r"\bR0UT3-4RB1T3R\b"), "internal monorepo name"),
    (re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"), "embedded private key"),
    (re.compile(r"\bAKIA[0-9A-Z]{16}\b"), "AWS access key id"),
    (re.compile(r"\bghp_[A-Za-z0-9]{36}\b"), "GitHub personal access token"),
    (re.compile(r"\bctx7sk[-_][A-Za-z0-9]+"), "context7 API key"),
]

# Text extensions to scan; binaries/images are skipped.
TEXT_SUFFIXES = {
    ".rs", ".toml", ".md", ".yml", ".yaml", ".sh", ".ps1", ".txt", ".json",
    ".svg", ".wxs", ".plist", ".rb", ".desktop", ".cfg", ".lock", "",
}


def should_skip(path: Path) -> bool:
    parts = set(path.relative_to(ROOT).parts)
    if parts & {".git", "target", "node_modules", "dist"}:
        return True
    if path.resolve() == SELF:
        return True
    if path.suffix.lower() not in TEXT_SUFFIXES:
        return True
    return False


def scan() -> list[str]:
    violations: list[str] = []
    for path in ROOT.rglob("*"):
        if not path.is_file() or should_skip(path):
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        for lineno, line in enumerate(text.splitlines(), start=1):
            for pattern, reason in FORBIDDEN:
                if pattern.search(line):
                    rel = path.relative_to(ROOT)
                    violations.append(f"{rel}:{lineno}: {reason}: {line.strip()[:120]}")
    return violations


def main() -> int:
    violations = scan()
    if violations:
        print(f"CONTENT-SAFETY AUDIT FAILED — {len(violations)} issue(s):", file=sys.stderr)
        for v in violations:
            print(f"  {v}", file=sys.stderr)
        return 1
    print("content-safety audit: OK (no internal leakage; safe for public repo)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
