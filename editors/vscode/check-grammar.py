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

# The suggestion list is the same kind of copy and goes stale the same way. A type the
# compiler has and the list does not is a type nobody is offered; one the list has and the
# compiler does not is a suggestion that will not compile.
completing = set(re.findall(r'\["([a-z0-9]+)",', (root / "editors/vscode/src/complete.js").read_text()))
types = set(re.findall(r'Ty::[A-Z]\w* => "([a-z0-9]+)"', (root / "crates/luarust-core/src/ty.rs").read_text()))
types.discard("nothing")

unoffered = sorted(types - completing)
invented = sorted(t for t in completing - types if re.fullmatch(r"(b|d|i|ui)\d+|er|bool|str", t))
if unoffered or invented:
    if unoffered:
        print("types the compiler has that completion never offers:", " ".join(unoffered))
    if invented:
        print("types completion offers that the compiler does not have:", " ".join(invented))
    print()
    print("the list is in editors/vscode/src/complete.js.")
    sys.exit(1)

print(f"completion offers all {len(types)} of the compiler's types and no others.")

# The project file's settings are enumerable, so the editor's table and the reader's can be
# compared outright: section, key and every value each key allows.
conf = (root / "crates/luarust-conf/src/lib.rs").read_text()
real_pairs = set(re.findall(r'\("([a-z]+)", "([a-z-]+)"\)', conf))
toml_js = (root / "editors/vscode/src/complete-toml.js").read_text()

offered_pairs = set()
for section in re.finditer(r"^  ([a-z]+): \{(.*?)^  \},", toml_js, re.S | re.M):
    for key in re.findall(r'^      "?([a-z-]+)"?: \{', section.group(2), re.M):
        offered_pairs.add((section.group(1), key))

if real_pairs != offered_pairs:
    missing = sorted(real_pairs - offered_pairs)
    extra = sorted(offered_pairs - real_pairs)
    if missing:
        print("settings `luarust-conf` reads that the editor never offers:", missing)
    if extra:
        print("settings the editor offers that `luarust-conf` does not read:", extra)
    print()
    print("the table is in editors/vscode/src/complete-toml.js.")
    sys.exit(1)

# Every value the editor suggests must be one the reader accepts.
accepted = set(re.findall(r'Some\("([a-z]+)"\) =>', conf)) | {"true", "false"}
suggested = set(re.findall(r'\[\'?"?\\?"?([a-z]+)\\?"?\'?, "', toml_js))
suggested = set(re.findall(r"\['\\?\"?([a-z]+)\\?\"?'", toml_js)) | set(re.findall(r'\["([a-z]+)",', toml_js))
invented = sorted(v for v in suggested - accepted if v not in {"what", "keys", "values"})
if invented:
    print("values the editor suggests that the project reader would refuse:", invented)
    sys.exit(1)

print(f"the project file's {len(real_pairs)} settings match `luarust-conf` exactly.")
