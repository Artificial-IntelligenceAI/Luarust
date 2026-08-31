#!/usr/bin/env python3
"""The manifest says what it must for the extension to actually do anything.

Highlighting is declarative and works whether or not the JavaScript ever runs, so a
missing activation event does not look broken -- the file is coloured, and completion and
errors are simply absent. That is a bad failure to have to notice by eye, so it is checked
instead.
"""

import json
import sys
from pathlib import Path

here = Path(__file__).resolve().parent
manifest = json.loads((here / "package.json").read_text())
wrong = []

languages = {entry["id"] for entry in manifest["contributes"]["languages"]}
events = set(manifest.get("activationEvents", []))
for language in languages:
    if f"onLanguage:{language}" not in events:
        wrong.append(f"nothing activates the extension for `{language}`; the JavaScript will not run")

for entry in manifest["contributes"]["grammars"]:
    if entry["language"] not in languages:
        wrong.append(f"a grammar names `{entry['language']}`, which is not a language here")
    if not (here / entry["path"]).exists():
        wrong.append(f"the grammar file {entry['path']} is not there")

for entry in manifest["contributes"]["languages"]:
    if "configuration" in entry and not (here / entry["configuration"]).exists():
        wrong.append(f"the language configuration {entry['configuration']} is not there")

main = manifest.get("main")
if not main or not (here / main).exists():
    wrong.append("`main` does not point at a file that exists")

declared = {c["command"] for c in manifest["contributes"]["commands"]}
registered = (here / "src/extension.js").read_text()
for command in declared:
    if f'"{command}"' not in registered:
        wrong.append(f"`{command}` is contributed but never registered")

if wrong:
    for line in wrong:
        print(" ", line)
    sys.exit(1)

print(f"the manifest activates for all {len(languages)} languages and every file it names is there.")
