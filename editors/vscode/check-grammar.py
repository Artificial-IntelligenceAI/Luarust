#!/usr/bin/env python3
"""Every word the compiler knows should be a word the editor colours.

The grammar is a copy of something the compiler already decides, and a copy left alone
goes stale quietly: a keyword gets added, nobody thinks about the extension, and it stops
looking like a keyword for a year. So the lists are compared rather than trusted.

Run from the root of the repository. Exits non-zero, and says which words, if any drifted.
"""

import json
import re
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[2]
grammar = json.loads((root / "editors/vscode/syntaxes/luarust.tmLanguage.json").read_text())

coloured = set()
for rule in grammar["repository"].values():
    for one in rule.get("patterns") or [rule]:
        coloured.update(re.findall(r"[a-z][a-z0-9-]+", one.get("match", "")))

# What the parser compares a bare word against, and every type's written name.
known = set(re.findall(r'"([a-z][a-z-]{1,14})"', (root / "crates/luarust-parse/src/lib.rs").read_text()))
known |= set(re.findall(r'"([a-z0-9]{2,5})"', (root / "crates/luarust-core/src/ty.rs").read_text()))

missing = sorted(word for word in known if word not in coloured)
if missing:
    print("the editor grammar does not colour words the compiler knows:")
    print("   ", " ".join(missing))
    print()
    print("add them to editors/vscode/syntaxes/luarust.tmLanguage.json, in the list they belong to.")
    sys.exit(1)

print(f"the grammar colours all {len(known)} words the compiler knows.")
