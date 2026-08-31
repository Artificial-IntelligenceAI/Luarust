// Colouring the lines a comment swallows.
//
// `#3` covers three lines and `#3d` covers four, and a TextMate grammar cannot express
// that: it matches within a line and has no way to reach forward. So the grammar colours
// the `#3` itself and this colours what it takes, which needs the file rather than a
// regular expression.
//
// The rule is the lexer's, and it is short enough to state exactly: digits straight after
// the `#`, then an optional `d`; the digits are how many lines counting the one it is
// written on, and the `d` makes them count downwards from it instead.

const vscode = require("vscode");

const KINDS = new vscode.SemanticTokensLegend(["comment"]);

/** Where each comment starts, and how many lines it covers. */
function commented(text) {
    const lines = text.split(/\r?\n/);
    const spans = [];

    for (let at = 0; at < lines.length; at++) {
        const line = lines[at];

        // Find a `#` that is not inside a name, a text, or a literal. The lexer decides
        // this the same way: quotes and bars open and close, and everything else is code.
        let name = false, quote = false, bar = false, hash = -1;
        for (let n = 0; n < line.length; n++) {
            const c = line[n];
            if (c === "'" && !quote && !bar) name = !name;
            else if (c === '"' && !name && !bar) quote = !quote;
            else if (c === "|" && !name && !quote) bar = !bar;
            else if (c === "#" && !name && !quote && !bar) { hash = n; break; }
        }
        if (hash < 0) continue;

        const counted = line.slice(hash + 1).match(/^(\d+)(d?)/);
        let covers = 1;
        if (counted) {
            const asked = Number(counted[1]);
            if (asked > 0) covers = counted[2] === "d" ? asked + 1 : asked;
        }

        // The `#` to the end of its own line, then whole lines after it. A comment that
        // runs off the end of the file simply stops there, as the lexer has it.
        spans.push({ line: at, from: hash, to: line.length });
        for (let n = 1; n < covers && at + n < lines.length; n++) {
            spans.push({ line: at + n, from: 0, to: lines[at + n].length });
        }
        at += covers - 1;
    }
    return spans;
}

function register(context) {
    context.subscriptions.push(
        vscode.languages.registerDocumentSemanticTokensProvider(
            { language: "luarust" },
            {
                provideDocumentSemanticTokens(document) {
                    const build = new vscode.SemanticTokensBuilder(KINDS);
                    for (const span of commented(document.getText())) {
                        if (span.to > span.from) {
                            build.push(span.line, span.from, span.to - span.from, 0, 0);
                        }
                    }
                    return build.build();
                },
            },
            KINDS,
        ),
    );
}

module.exports = { register, commented };
