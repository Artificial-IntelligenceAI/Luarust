// The suggestion list offers each word once, and never a word twice.
//
// `var`, `fn`, `loop`, `if` and `print` are each a statement and a snippet, and for a
// while the list held both -- two rows with the same word, the same look and no way to
// tell which would insert what. Nothing about that is visible from reading the source,
// so it is checked.

const Module = require("module");
const real = Module._load;
Module._load = function (request, ...rest) {
  if (request === "vscode") {
    return {
      CompletionItem: class { constructor(label, kind) { this.label = label; this.kind = kind; } },
      CompletionItemKind: { Keyword: 1, TypeParameter: 2, Variable: 3, Snippet: 4, Module: 5, Property: 6, Value: 7 },
      SnippetString: class { constructor(value) { this.value = value; } },
      MarkdownString: class { constructor(value) { this.value = value; } },
      languages: { registerCompletionItemProvider(_, provider) { providers.push(provider); } },
      workspace: { getConfiguration: () => ({ get: (_, fallback) => fallback }) },
    };
  }
  return real(request, ...rest);
};

const providers = [];
require("./src/complete.js").register({ subscriptions: [] });
require("./src/complete-toml.js").register({ subscriptions: [] });

const wrong = [];

function labelsOf(items) {
  return (items || []).map((i) => i.label);
}

function noRepeats(where, labels) {
  const seen = new Set();
  for (const label of labels) {
    if (seen.has(label)) wrong.push(`${where} offers \`${label}\` more than once`);
    seen.add(label);
  }
}

const lr = providers[0];
const blank = { lineAt: () => ({ text: "" }), getText: () => "" };
noRepeats("the general list", labelsOf(lr.provideCompletionItems(blank, { line: 0, character: 0 })));

for (const chain of ["var.", "var.local.", "var.local.mut.", "fn.", "fn.local.", "loop.", "loop.temp."]) {
  const document = { lineAt: () => ({ text: chain }), getText: () => "" };
  noRepeats(`after \`${chain}\``, labelsOf(lr.provideCompletionItems(document, { line: 0, character: chain.length })));
}

// Every snippet must say what it inserts, or it looks like the bare word beside it.
for (const item of lr.provideCompletionItems(blank, { line: 0, character: 0 })) {
  if (item.kind === 4 && !item.documentation) {
    wrong.push(`the \`${item.label}\` snippet does not show what it inserts`);
  }
}

const toml = providers[1];
const lines = ["[gc]", ""];
const document = { lineCount: 2, lineAt: (n) => ({ text: lines[typeof n === "number" ? n : n.line] }) };
noRepeats("the project file's keys", labelsOf(toml.provideCompletionItems(document, { line: 1, character: 0 })));

if (wrong.length) {
  for (const line of wrong) console.log(" ", line);
  process.exit(1);
}
console.log("every suggestion list offers each word exactly once.");
