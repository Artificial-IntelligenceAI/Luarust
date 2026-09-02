// Suggestions for `Luarust.toml`, which has four sections and eight settings.
//
// A project file is small and completely enumerable, so there is no reason for the editor
// to be vaguer about it than the compiler is. Every section, every key and every value it
// may take is here, and `check-grammar.py` fails if this table and `luarust-conf` ever
// disagree about any of them.

const vscode = require("vscode");

const SETTINGS = {
  defaults: {
    what: "applies to every file in the project",
    keys: {
      overflow: {
        what: "what an integer does when it will not fit",
        values: [
          ['"wrap"', "it wraps round, the way the hardware does"],
          ['"trap"', "it stops the program and says so"],
        ],
      },
      "float-printing": {
        what: "how much of a binary float a program writes out",
        values: [
          ['"exact"', "the value it holds, whole -- `0.1` is not one tenth and says so"],
          ['"shortest"', "the fewest digits that name it and no other, at its own format"],
        ],
      },
      division: {
        what: "which way `div` rounds, and which way `mod` leans with it",
        values: [
          ['"floored"', "the remainder follows the divisor -- Knuth's, and Python's"],
          ['"truncated"', "the remainder follows the dividend -- C's, and the hardware's"],
          ['"euclidean"', "the remainder is never negative"],
        ],
      },
      "no-visibility-stated": {
        what: "what a declaration means when it says no visibility",
        values: [
          ['"restricted"', "it exists and nothing may touch it"],
          ['"error"', "the declaration is refused on the spot"],
        ],
      },
    },
  },
  build: {
    what: "what gets delivered, rather than what gets accepted",
    keys: {
      "embed-source": {
        what: "whether a chunk carries the text it was built from",
        values: [
          ["true", "faults can quote the line"],
          ["false", "the line table only -- four bytes a line, and faults still point"],
        ],
      },
      "target-cpu": {
        what: "which machine `luarust native` is building a program for",
        values: [
          ['"portable"', "everything the architecture guarantees -- runs anywhere it does"],
          ['"this-machine"', "everything the builder has -- faster, and needs a processor as capable"],
        ],
      },
      "decimal-encoding": {
        what: "which of IEEE 754's two ways of writing a decimal significand a chunk uses",
        values: [
          ['"bid"', "binary integer decimal"],
          ['"dpd"', "densely packed decimal"],
        ],
      },
    },
  },
  run: {
    what: "how a chunk is run, once it is one",
    keys: {
      mode: {
        what: "which engine runs it",
        values: [
          ['"vm"', "the bytecode, interpreted -- nothing is compiled and nothing is spent compiling"],
          ['"whole"', "all of it through LLVM before it starts -- full speed from the first iteration"],
          ['"hot"', "interpreted, and a loop that proves itself is compiled and jumped into"],
        ],
      },
      engine: {
        what: "how hard the project insists on having that engine",
        values: [
          ['"optional"', "no JIT on the machine? run on the VM, and say so -- the default"],
          ['"required"', "no JIT on the machine? refuse to run rather than run it slowly"],
          ['"bundled"', "`luarust build` puts a runtime that has the engine beside the chunk"],
        ],
      },
    },
  },
  gc: {
    what: "whether a running program collects the arrays nothing can reach",
    keys: {
      mode: {
        what: "how eagerly to collect, if at all",
        values: [
          ['"off"', "never -- and no collector goes into the program at all"],
          ['"silent"', "when a megabyte has been handed out since the last time"],
          ['"aggressive"', "every four kilobytes -- the smallest heap, and slower"],
        ],
      },
    },
  },
};

function item(label, detail, kind) {
  const made = new vscode.CompletionItem(label, kind);
  made.detail = detail;
  return made;
}

/** Which section the cursor is in, by looking upwards for the nearest header. */
function sectionAt(document, line) {
  for (let n = line; n >= 0; n--) {
    const header = document.lineAt(n).text.match(/^\s*\[\s*([a-z-]+)\s*\]/);
    if (header) return header[1];
  }
  return null;
}

/** Keys already written in this section, so a setting is not offered twice. */
function alreadySet(document, line) {
  const set = new Set();
  for (let n = line; n >= 0; n--) {
    const text = document.lineAt(n).text;
    if (/^\s*\[/.test(text) && n !== line) break;
    const key = text.match(/^\s*([a-z-]+)\s*=/);
    if (key && n !== line) set.add(key[1]);
  }
  for (let n = line + 1; n < document.lineCount; n++) {
    const text = document.lineAt(n).text;
    if (/^\s*\[/.test(text)) break;
    const key = text.match(/^\s*([a-z-]+)\s*=/);
    if (key) set.add(key[1]);
  }
  return set;
}

function suggest(document, position) {
  const before = document.lineAt(position).text.slice(0, position.character);
  if (/#/.test(before)) return undefined;

  // A value: the key to the left settles what may be written.
  const assigning = before.match(/^\s*([a-z-]+)\s*=\s*"?[a-z]*$/);
  if (assigning) {
    const section = sectionAt(document, position.line);
    const key = section && SETTINGS[section]?.keys[assigning[1]];
    if (!key) return undefined;
    // If a quote has been typed already, do not offer another one.
    const quoted = /"/.test(before);
    return key.values.map(([v, d]) =>
      item(quoted ? v.replace(/"/g, "") : v, d, vscode.CompletionItemKind.Value),
    );
  }

  // A section header.
  if (/^\s*\[[a-z-]*$/.test(before)) {
    return Object.entries(SETTINGS).map(([name, s]) =>
      item(name, s.what, vscode.CompletionItemKind.Module),
    );
  }

  // A bare word at the start of a line: the keys of whichever section this is.
  if (/^\s*[a-z-]*$/.test(before)) {
    const section = sectionAt(document, position.line);
    if (!section || !SETTINGS[section]) {
      // Before any header, the only thing that can come next is a header.
      return Object.entries(SETTINGS).map(([name, s]) =>
        item(`[${name}]`, s.what, vscode.CompletionItemKind.Module),
      );
    }
    const set = alreadySet(document, position.line);
    return Object.entries(SETTINGS[section].keys)
      .filter(([name]) => !set.has(name))
      .map(([name, k]) => {
        const made = item(name, k.what, vscode.CompletionItemKind.Property);
        made.insertText = new vscode.SnippetString(`${name} = \${1|${k.values.map(([v]) => v).join(",")}|}`);
        return made;
      });
  }

  return undefined;
}

function register(context) {
  context.subscriptions.push(
    vscode.languages.registerCompletionItemProvider(
      "luarust-toml",
      {
        provideCompletionItems(document, position) {
          if (!vscode.workspace.getConfiguration("luarust").get("suggest", true)) {
            return undefined;
          }
          return suggest(document, position);
        },
      },
      "[",
      "=",
      '"',
    ),
  );
}

module.exports = { register, suggest, SETTINGS };
