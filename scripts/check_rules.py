#!/usr/bin/env python3
"""Mechanical checks of ACCEPTANCE.md: A1, A2, A8, D2, D4.

Usage:
  scripts/check_rules.py                  check; exit 1 on a violation
  scripts/check_rules.py --write-baseline regenerate scripts/size-baseline.txt from the tree
                                          (only in a commit that moves or shrinks code; the
                                          totals it prints must not grow)

The baseline lists what is over budget today (files over 800 lines, functions over 80 lines,
files with `.unwrap()` outside tests). Anything not listed must meet the budget; anything
listed may shrink but not grow.
"""
import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASELINE = os.path.join(ROOT, "scripts", "size-baseline.txt")
FILE_BUDGET = 800
FN_BUDGET = 80

# Source trees scanned. Crates appear as the workspace split lands; until then `src/` is all.
SOURCE_DIRS = ["src", "crates", "addons"]
# A1: the core crates, which must stay generic.
CORE_DIRS = ["crates/core", "crates/tui"]
A1_PATTERN = re.compile(r"vocdoni|claude-proxy|claude\.vocdoni", re.IGNORECASE)
# A2: workspace packages and what they may depend on.
CORE = "claudio-core"
TUI = "claudio-tui"


def rust_files():
    for base in SOURCE_DIRS:
        top = os.path.join(ROOT, base)
        if not os.path.isdir(top):
            continue
        for dirpath, dirnames, filenames in os.walk(top):
            dirnames[:] = [d for d in dirnames if d not in ("target", "node_modules", "vendor")]
            for name in filenames:
                if name.endswith(".rs"):
                    yield os.path.relpath(os.path.join(dirpath, name), ROOT)


def is_test_file(path):
    parts = path.split(os.sep)
    # `src/daemon/tests.rs`, `src/daemon/tests/*.rs`, a crate's `tests/` dir, test_support.
    return "tests" in parts[:-1] or parts[-1] in ("tests.rs", "test_support.rs")


def production_lines(text):
    """The file's lines before its top-level `#[cfg(test)] mod … {` (the unit tests)."""
    lines = text.split("\n")
    for i, line in enumerate(lines[:-1]):
        if line.startswith("#[cfg(test)]") and re.match(r"(pub(\([^)]*\))?\s+)?mod \w+\s*\{", lines[i + 1]):
            return lines[:i]
    return lines


FN_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?"
                   r"(?:extern\s+\"[^\"]*\"\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)")


def strip_noise(line):
    """A line without string/char literals and `//` comments, for brace counting."""
    line = re.sub(r'r#*"[^"]*"#*', '""', line)
    line = re.sub(r'"(?:\\.|[^"\\])*"', '""', line)
    line = re.sub(r"'(?:\\.|[^'\\])'", "''", line)
    i = line.find("//")
    return line if i < 0 else line[:i]


def functions(lines):
    """(name, length) of every fn with a body, by brace matching (a heuristic, but stable)."""
    out = []
    n = len(lines)
    for i in range(n):
        m = FN_RE.match(lines[i])
        if not m:
            continue
        depth = 0
        opened = False
        for j in range(i, n):
            code = strip_noise(lines[j])
            if not opened and ";" in code and "{" not in code:
                break  # a declaration without a body (trait method)
            for ch in code:
                if ch == "{":
                    depth += 1
                    opened = True
                elif ch == "}":
                    depth -= 1
            if opened and depth <= 0:
                out.append((m.group(1), j - i + 1))
                break
    return out


def measure():
    """Everything over budget ({key: value}) and the A1/D2/D4 violations."""
    over = {}
    hard = []
    for path in sorted(rust_files()):
        text = open(os.path.join(ROOT, path), encoding="utf-8").read()
        if any(path.startswith(d + os.sep) for d in CORE_DIRS):
            for k, line in enumerate(text.split("\n"), 1):
                if A1_PATTERN.search(line):
                    hard.append(f"A1 {path}:{k}: the core names a team service: {line.strip()[:80]}")
        if is_test_file(path):
            continue
        prod = production_lines(text)
        if len(prod) > FILE_BUDGET:
            over[f"file {path}"] = len(prod)
        seen = {}
        for name, length in functions(prod):
            seen[name] = seen.get(name, 0) + 1
            if length > FN_BUDGET:
                key = f"fn {path}::{name}" + (f"#{seen[name]}" if seen[name] > 1 else "")
                over[key] = length
        unwraps = sum(line.count(".unwrap()") for line in prod)
        if unwraps:
            over[f"unwrap {path}"] = unwraps
        for k, line in enumerate(prod, 1):
            if re.search(r"\b(todo|unimplemented|dbg)!\s*\(", strip_noise(line)):
                hard.append(f"D2 {path}:{k}: {line.strip()[:80]}")
            if re.search(r"#\[allow\((dead_code|unused[a-z_]*)", line):
                prev = prod[k - 2] if k >= 2 else ""
                if "//" not in line and not prev.strip().startswith("//"):
                    hard.append(f"D4 {path}:{k}: an allow without a comment saying why")
    return over, hard


def dependency_violations():
    try:
        meta = subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--no-deps", "--offline"],
            cwd=ROOT, capture_output=True, text=True, check=True,
        )
    except (OSError, subprocess.CalledProcessError) as e:
        return [f"A2 cargo metadata failed: {e}"]
    packages = json.loads(meta.stdout)["packages"]
    names = {p["name"] for p in packages}
    bad = []
    for p in packages:
        me = p["name"]
        deps = {d["name"] for d in p["dependencies"] if d["name"] in names}
        if me == CORE:
            allowed = set()
        elif me == TUI:
            allowed = {CORE}
        elif me == "claudio":
            allowed = names - {me}
        else:  # an addon
            allowed = {CORE, TUI}
        for d in sorted(deps - allowed):
            bad.append(f"A2 {me} depends on {d}")
    return bad


def read_baseline():
    base = {}
    if os.path.exists(BASELINE):
        for line in open(BASELINE, encoding="utf-8"):
            line = line.strip()
            if line and not line.startswith("#"):
                key, _, value = line.rpartition(" ")
                base[key] = int(value)
    return base


def totals(entries):
    t = {"file": 0, "fn": 0, "unwrap": 0}
    for key, value in entries.items():
        t[key.split(" ", 1)[0]] += value
    return t


def main():
    over, hard = measure()
    if "--write-baseline" in sys.argv:
        before = totals(read_baseline())
        with open(BASELINE, "w", encoding="utf-8") as f:
            f.write("# Over-budget items (ACCEPTANCE.md A8, D2). Only shrinks; see scripts/check_rules.py.\n")
            for key in sorted(over):
                f.write(f"{key} {over[key]}\n")
        print(f"baseline written: {len(over)} items; totals before {before}, now {totals(over)}")
        return 0
    base = read_baseline()
    problems = list(hard) + dependency_violations()
    for key, value in sorted(over.items()):
        kind, _, item = key.partition(" ")
        rule = "D2" if kind == "unwrap" else "A8"
        limit = base.get(key)
        if limit is None:
            budget = {"file": FILE_BUDGET, "fn": FN_BUDGET, "unwrap": 0}[kind]
            problems.append(f"{rule} {item}: {value} (budget {budget})")
        elif value > limit:
            problems.append(f"{rule} {item}: grew from {limit} to {value}")
    for p in problems:
        print(p)
    t = totals(over)
    print(f"rules: {len(problems)} problem(s); over budget: {t['file']} file lines, "
          f"{t['fn']} fn lines, {t['unwrap']} unwraps")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
