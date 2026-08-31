// The editor greys exactly the lines the compiler comments out.
//
// The provider re-implements a rule the lexer owns -- `#3` covers three lines, `#3d`
// four -- which is a copy, and a copy drifts. So it is checked against the compiler
// itself: a file where every line prints its own number, run for real, and what came out
// is exactly the lines the provider left alone.

const { execFileSync } = require("child_process");
const fs = require("fs");
const os = require("os");
const path = require("path");

const Module = require("module");
const real = Module._load;
Module._load = function (request, ...rest) {
  if (request === "vscode") {
    return {
      SemanticTokensLegend: class { constructor(kinds) { this.kinds = kinds; } },
      SemanticTokensBuilder: class { constructor() {} push() {} build() {} },
      languages: { registerDocumentSemanticTokensProvider() {} },
    };
  }
  return real(request, ...rest);
};
const { commented } = require("./src/comments.js");

const luarust = process.argv[2] || "target/release/luarust";
if (!fs.existsSync(luarust)) {
  console.log(`no ${luarust} to check against; build it first`);
  process.exit(0);
}

// Every shape the rule has: bare, counted, counted-down, trailing after code, a number
// that is a remark rather than a count, and one that runs off the end.
const lines = [
  "#",
  'print["1" \\n];',
  "#3",
  'print["3" \\n];',
  'print["4" \\n];',
  'print["5" \\n];',
  "#2d",
  'print["7" \\n];',
  'print["8" \\n];',
  'print["9" \\n];',
  'print["10" \\n]; #2',
  'print["11" \\n];',
  'print["12" \\n];',
  "# 3 things",
  'print["14" \\n];',
  '#1 var.local.str [\'a\'] = ["# not a comment"];',
  'print["16" \\n];',
];
const source = lines.join("\n") + "\n";

const file = path.join(fs.mkdtempSync(path.join(os.tmpdir(), "luarust-")), "c.lr");
fs.writeFileSync(file, source);
const printed = execFileSync(luarust, ["run", file], { encoding: "utf8" })
  .split("\n")
  .filter(Boolean)
  .map(Number);

// Greyed from the first column is a line that is *all* comment. Greyed from further in
// is a remark after code that still runs -- `print["10" \n]; #2` is both.
const spans = commented(source);
const whole = new Set(spans.filter((s) => s.from === 0).map((s) => s.line));

const wrong = [];
for (const n of printed) {
  if (whole.has(n)) wrong.push(`line ${n} ran, and the editor greys the whole of it`);
}
for (const [n, line] of lines.entries()) {
  if (/^\s*print\[/.test(line) && !printed.includes(n) && !whole.has(n)) {
    wrong.push(`line ${n} was commented out, and the editor leaves it lit`);
  }
}

if (wrong.length) {
  for (const line of wrong) console.log(" ", line);
  console.log("\n  the rule is the lexer's; the copy is in editors/vscode/src/comments.js.");
  process.exit(1);
}
console.log(`the editor greys exactly what the compiler comments out (${printed.length} lines ran).`);
