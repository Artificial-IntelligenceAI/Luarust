// The extension is the compiler's errors, put where VS Code shows errors.
//
// Nothing here decides whether a program is right. `luarust check` decides that, and this
// reads what it said. A second opinion written in JavaScript would be a second thing to
// keep correct, and it would be wrong in ways the real one is not.

const { spawn } = require("child_process");
const vscode = require("vscode");
const complete = require("./complete");

/** Where `luarust` is, as the settings have it. */
function command() {
  return vscode.workspace.getConfiguration("luarust").get("path", "luarust");
}

/**
 * Pull diagnostics out of what the compiler printed.
 *
 * A fault is written as a location line, a blank line, the message, and then the quoted
 * source and the codes underneath:
 *
 *     file: src/main.lr, line: 3, column: 25 (src/main.lr:3:25)
 *
 *     this does not fit in `ui8`.
 *
 *       3 | var.local.ui8 ['n'] = [|300|];
 *         |                        ^^^^^ written here
 *
 *     Error code: E0203
 *     Rule(s) broken: a value has to fit the type it is given
 *     Suggested fix(s): give it a type that holds 300.
 *
 * The caret line gives the width, so a squiggle covers exactly what the compiler
 * underlined rather than guessing at a word boundary.
 */
function parse(output) {
  const lines = output.split("\n");
  const found = [];

  for (let i = 0; i < lines.length; i++) {
    const where = lines[i].match(/^file: .*, line: (\d+), column: (\d+)/);
    if (!where) continue;

    const line = Number(where[1]) - 1;
    const column = Number(where[2]) - 1;

    // The message is the next line with anything on it.
    let message = "";
    for (let n = i + 1; n < lines.length && n < i + 5; n++) {
      if (lines[n].trim()) { message = lines[n].trim(); break; }
    }
    if (!message) continue;

    // How much was underlined, if the source was quoted. Without it, one character.
    let width = 1;
    for (let n = i + 1; n < lines.length && n < i + 12; n++) {
      const carets = lines[n].match(/^\s*\|\s*(\^+)/);
      if (carets) { width = carets[1].length; break; }
      if (/^file: /.test(lines[n])) break;
    }

    // The code, the rule and the fix, gathered for the hover.
    const extra = [];
    let code = "";
    for (let n = i + 1; n < lines.length && n < i + 16; n++) {
      if (/^file: /.test(lines[n])) break;
      const found_code = lines[n].match(/^Error code: (\S+)/);
      if (found_code) { code = found_code[1]; continue; }
      if (/^(Rule\(s\) broken|Tip\(s\)|Suggested fix\(s\)):/.test(lines[n])) {
        extra.push(lines[n].trim());
      }
    }

    const range = new vscode.Range(line, column, line, column + width);
    const whole = extra.length ? `${message}\n${extra.join("\n")}` : message;
    const diagnostic = new vscode.Diagnostic(range, whole, vscode.DiagnosticSeverity.Error);
    diagnostic.source = "luarust";
    if (code) diagnostic.code = code;
    found.push(diagnostic);
  }
  return found;
}

/** Run `luarust check` over one file and show whatever it says. */
function check(document, collection, output) {
  if (document.languageId !== "luarust") return;

  const child = spawn(command(), ["check", document.fileName], {
    cwd: vscode.workspace.getWorkspaceFolder(document.uri)?.uri.fsPath,
  });

  let said = "";
  child.stdout.on("data", (d) => (said += d));
  child.stderr.on("data", (d) => (said += d));

  child.on("error", (why) => {
    // Said once, not once per keystroke: a missing compiler is a settings problem, and
    // repeating it in a popup while somebody types is no help at all.
    output.appendLine(`could not run \`${command()}\`: ${why.message}`);
    output.appendLine("set `luarust.path` to where the binary is.");
    collection.set(document.uri, []);
  });

  child.on("close", () => collection.set(document.uri, parse(said)));
}

/** Run the file, whichever way was asked for, in a terminal the user can see. */
function runIn(way) {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== "luarust") {
    vscode.window.showInformationMessage("Open a `.lr` file first.");
    return;
  }
  editor.document.save().then(() => {
    const terminal =
      vscode.window.terminals.find((t) => t.name === "Luarust") ||
      vscode.window.createTerminal("Luarust");
    terminal.show();
    terminal.sendText(`${command()} ${way} ${quote(editor.document.fileName)}`);
  });
}

/** A path with a space in it is still one argument. */
function quote(path) {
  return /[^\w./-]/.test(path) ? `'${path.replace(/'/g, "'\\''")}'` : path;
}

function activate(context) {
  const collection = vscode.languages.createDiagnosticCollection("luarust");
  const output = vscode.window.createOutputChannel("Luarust");
  context.subscriptions.push(collection, output);

  const when = () =>
    vscode.workspace.getConfiguration("luarust").get("checkOn", "save");

  const now = (document) => check(document, collection, output);

  context.subscriptions.push(
    vscode.workspace.onDidSaveTextDocument((d) => when() !== "never" && now(d)),
    vscode.workspace.onDidOpenTextDocument((d) => when() !== "never" && now(d)),
    vscode.workspace.onDidCloseTextDocument((d) => collection.delete(d.uri)),
  );

  // Checking as you type means checking a file that is half-written, so it waits for a
  // pause rather than running on the keystroke.
  let waiting;
  context.subscriptions.push(
    vscode.workspace.onDidChangeTextDocument((event) => {
      if (when() !== "type") return;
      clearTimeout(waiting);
      waiting = setTimeout(() => now(event.document), 400);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("luarust.check", () => {
      const editor = vscode.window.activeTextEditor;
      if (editor) now(editor.document);
    }),
    vscode.commands.registerCommand("luarust.run", () => runIn("run")),
    vscode.commands.registerCommand("luarust.jit", () => runIn("jit")),
    vscode.commands.registerCommand("luarust.dis", () => runIn("dis")),
  );

  complete.register(context);

  vscode.workspace.textDocuments.forEach((d) => when() !== "never" && now(d));
}

function deactivate() {}

module.exports = { activate, deactivate, parse };
