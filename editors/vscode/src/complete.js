// Suggestions, following the chain the language is built out of.
//
// A Luarust declaration is a chain read left to right -- `var.local.mut.ui32` -- and each
// dot has a small, known set of things that may come next. That is what makes completion
// worth having here rather than a list of every keyword at every position: after `var.`
// there are four answers, and after those four there is `mut` or a type. The suggestion
// list can be exactly right instead of merely long.

const vscode = require("vscode");

const VISIBILITY = [
  ["local", "the block it is written in, and no further"],
  ["global", "the whole program"],
  ["public", "the whole program, and exported so importers see it too"],
  ["restricted", "nobody, anywhere, on purpose -- every use of it is an error"],
];

const TYPES = [
  ["b16", "IEEE 754 binary16 -- half precision"],
  ["b32", "IEEE 754 binary32 -- what most languages call `float`"],
  ["b64", "IEEE 754 binary64 -- what most languages call `double`"],
  ["b128", "IEEE 754 binary128 -- no hardware has it; `luarust-num` does"],
  ["b256", "IEEE 754 binary256 -- almost nothing on earth implements it"],
  ["d32", "IEEE 754 decimal32 -- a tenth is a tenth"],
  ["d64", "IEEE 754 decimal64 -- where money keeps its cents"],
  ["d128", "IEEE 754 decimal128"],
  ["er", "an exact rational -- never rounds, never overflows"],
  ["i8", "signed, -128 to 127"],
  ["i16", "signed, 16 bits"],
  ["i32", "signed, 32 bits"],
  ["i64", "signed, 64 bits"],
  ["ui8", "unsigned, 0 to 255"],
  ["ui16", "unsigned, 16 bits"],
  ["ui32", "unsigned, 32 bits"],
  ["ui64", "unsigned, 64 bits"],
  ["bool", "`true` or `false`, and nothing else is either"],
  ["str", "text"],
];

const STATEMENTS = [
  ["var", "declare something"],
  ["fn", "declare a function"],
  ["set", "change something already declared"],
  ["print", "write to the output"],
  ["loop", "go round"],
  ["if", "decide"],
  ["return", "answer, from inside a function"],
  ["break", "leave the loop"],
  ["math", "an expression, in a block of its own"],
  ["defaults", "a setting, for this file"],
];

/** A completion item, with the description shown beside it. */
function item(label, detail, kind) {
  const made = new vscode.CompletionItem(label, kind);
  made.detail = detail;
  return made;
}

const types = () => TYPES.map(([w, d]) => item(w, d, vscode.CompletionItemKind.TypeParameter));
const visibility = () =>
  VISIBILITY.map(([w, d]) => item(w, d, vscode.CompletionItemKind.Keyword));

/** Every name the file declares, so `'` offers what is actually there. */
function declared(document) {
  const names = new Set();
  const text = document.getText();
  // `var.local.ui8 ['n']`, `fn.local.ui64 ['factorial']`, and a loop's counter.
  for (const found of text.matchAll(/(?:var|fn|loop)[a-z0-9.-]*\s*\[\s*'([^']+)'/g)) {
    names.add(found[1]);
  }
  // A function's parameters: `[ui64 'n', str 'what']`.
  for (const found of text.matchAll(/[a-z0-9]+\s+'([^']+)'/g)) {
    names.add(found[1]);
  }
  return [...names].map((n) => item(n, "declared in this file", vscode.CompletionItemKind.Variable));
}

/** A whole construct, written out with the holes to fill in. */
function snippet(label, detail, body) {
  const made = new vscode.CompletionItem(label, vscode.CompletionItemKind.Snippet);
  made.detail = detail;
  made.insertText = new vscode.SnippetString(body);
  // What it will actually put in the file, so a snippet and a bare word are told apart by
  // reading rather than by noticing which icon is beside them.
  made.documentation = new vscode.MarkdownString("```luarust\n" + plain(body) + "\n```");
  return made;
}

/** A snippet body as the text it starts out as, with the holes shown filled in. */
function plain(body) {
  return body
    // `${1|a,b|}` -- a choice, shown as the first of them.
    .replace(/\$\{\d+\|([^|]*)\|\}/g, (_, choices) => choices.split(",")[0])
    // `${1:name}` -- a hole with something written in it already.
    .replace(/\$\{\d+:([^}]*)\}/g, "$1")
    // `$0`, `$1` -- a hole with nothing in it.
    .replace(/\$\d+/g, "")
    .replace(/\t/g, "    ");
}

const SNIPPETS = () => [
  snippet("var", "a declaration", "var.local.${1|mut.,|}${2:ui32} ['${3:name}'] = [|${4:0}|];"),
  snippet("fn", "a function", "fn.local.${1:ui64} ['${2:name}'] [${3:ui64 'n'}] {\n\t$0\n}"),
  snippet("loop", "a counted loop", "loop.temp.range.${1:ui32} ['${2:i}'] = [|${3:1}|, |${4:10}|] {\n\t$0\n}"),
  snippet("while", "a loop with a condition", "loop.perm.while.${1:ui8} ['${2:n}'] [math { ${3:condition} }] {\n\t$0\n\tbreak when reached |${4:100}|;\n}"),
  snippet("if", "a decision", "if [math { ${1:condition} }] {\n\t$0\n}"),
  snippet("print", "write a line", "print[${1:\"text\"} \\n];"),
  // Written with `filled`, not written out: a fixed array's length is in its type, so a
  // snippet with the length in one hole and the elements in another compiles only while
  // the two agree, and the first thing anybody does to a snippet is change a hole.
  snippet("array", "an array, filled", "var.local.array.${1:ui32} ['${2:name}'] = [filled[|${3:3}|, |${4:0}|]];"),
  snippet("array-written", "an array, written out", "var.local.array.3.${1:ui32} ['${2:name}'] = [[|${3:1}|, |${4:2}|, |${5:3}|]];"),
];

/**
 * What may come next, given what is already to the left of the cursor.
 *
 * The chain is read left to right, so the answer is decided by counting the dots that
 * have been typed rather than by guessing from the whole line.
 */
function after(before) {
  const chain = before.match(/(?:^|[\s;{}[\]])(var|fn|loop)((?:\.[a-z0-9-]*)*)$/);
  if (!chain) return null;

  const [, word, rest] = chain;
  // "" for `var`, ["local"] for `var.local`, ["local",""] for `var.local.`
  const parts = rest ? rest.slice(1).split(".") : null;
  if (parts === null) return null;
  const settled = parts.slice(0, -1);

  if (word === "loop") {
    if (settled.length === 0) {
      return [
        item("temp", "the counter is gone after the loop", vscode.CompletionItemKind.Keyword),
        item("perm", "the counter outlives the loop", vscode.CompletionItemKind.Keyword),
      ];
    }
    if (settled.length === 1) {
      return [
        item("range", "count from one bound to another", vscode.CompletionItemKind.Keyword),
        item("while", "go round for as long as something holds", vscode.CompletionItemKind.Keyword),
      ];
    }
    return types();
  }

  if (settled.length === 0) return visibility();
  if (word === "fn") return types();

  // `var`: after the visibility comes `mut`, or the type, or `array`.
  if (settled.length === 1) {
    return [
      item("mut", "it may be changed after it is declared", vscode.CompletionItemKind.Keyword),
      item("array", "an array of them", vscode.CompletionItemKind.Keyword),
      ...types(),
    ];
  }
  if (settled[settled.length - 1] === "array" || settled[1] === "mut") {
    return [item("array", "an array of them", vscode.CompletionItemKind.Keyword), ...types()];
  }
  return types();
}

function register(context) {
  context.subscriptions.push(
    vscode.languages.registerCompletionItemProvider(
      "luarust",
      {
        provideCompletionItems(document, position) {
          if (!vscode.workspace.getConfiguration("luarust").get("suggest", true)) {
            return undefined;
          }
          const before = document.lineAt(position).text.slice(0, position.character);

          // Inside a comment, nothing. Inside text, nothing either.
          if (/--/.test(before)) return undefined;
          if ((before.match(/"/g) || []).length % 2 === 1) return undefined;

          // Halfway through a name: offer the names this file declares.
          if ((before.match(/'/g) || []).length % 2 === 1) return declared(document);

          const chained = after(before);
          if (chained) return chained;

          // One entry per word. `var`, `fn`, `loop`, `if` and `print` are each a
          // statement and a snippet, and offering both put two identical-looking rows in
          // the list with no way to tell which was which. The snippet wins: it is the
          // same word plus the rest of the construct, and typing the `.` after it opens
          // the chain list anyway.
          const written = new Set(SNIPPETS().map((s) => s.label));
          return [
            ...SNIPPETS(),
            ...STATEMENTS.filter(([w]) => !written.has(w)).map(([w, d]) =>
              item(w, d, vscode.CompletionItemKind.Keyword),
            ),
            ...types(),
          ];
        },
      },
      ".",
      "'",
    ),
  );
}

module.exports = { register, after, declared };
