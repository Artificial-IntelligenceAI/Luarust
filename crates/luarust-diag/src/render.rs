//! Laying an error out the way Luarust says them.
//!
//! ```text
//! Hello, I think there may be thing(s) wrong with your code. I'm sorry, if I'm wrong.
//!
//! file: /Users/ts/hello/src/main.lr, line: 4, column: 6 (src/main.lr:4:6)
//!
//! `'total'` cannot be changed, because its declaration never said it could.
//!
//!   2 | var.local.ui32 ['total'] = [|0|];
//!     |     ~~~~~ declared here, and `mut` is not in the chain
//!   4 | set ['total'] = [|55|];
//!     |      ^^^^^^^ changed here
//!
//! Error code: E0104
//! Rule(s) broken: a variable changes only if its declaration says `mut`
//! Tip(s): `mut` goes between the visibility and the type.
//! Suggested fix(s): line 2 — `var.local.mut.ui32 ['total'] = [|0|];`
//!
//! 1 error.
//! ```
//!
//! The greeting is printed once however many errors follow, and the count once at the
//! end, so a program with twelve mistakes apologises once rather than twelve times.
//!
//! The fix is last on purpose: it is what should still be on screen when the reader stops
//! reading.

use crate::diag::{Diagnostic, LabelStyle};
use crate::source::SourceFile;
use std::fmt::Write as _;

/// The apology, printed once above however many errors follow it.
pub const GREETING: &str =
    "Hello, I think there may be thing(s) wrong with your code. I'm sorry, if I'm wrong.";

/// Render one diagnostic, without the greeting or the count.
pub fn diagnostic(source: &SourceFile, diag: &Diagnostic) -> String {
    let mut out = String::new();

    // Where it is, in the two forms: one to read, one to paste.
    if let Some(label) = diag.primary_label() {
        let at = source.position(label.span.start);
        let _ = writeln!(
            out,
            "file: {}, line: {}, column: {} ({})",
            source.path().display(),
            at.line,
            at.column,
            source.short_location(label.span.start),
        );
        out.push('\n');
    }

    let _ = writeln!(out, "{}", diag.message);

    if !diag.labels.is_empty() {
        out.push('\n');
        if source.has_text() {
            out.push_str(&snippet(source, diag));
        } else {
            // The line is known and the line cannot be shown. Say which, rather than
            // printing an empty frame and letting it look like an empty line.
            let _ = writeln!(
                out,
                "  (this was built without its source, so the line above cannot be shown.)\n"
            );
        }
    }

    out.push('\n');
    let _ = writeln!(out, "Error code: {}", diag.code);
    write_field(&mut out, "Rule(s) broken", &diag.rules);
    write_field(&mut out, "Tip(s)", &diag.tips);
    write_field(&mut out, "Suggested fix(s)", &diag.fixes);

    out
}

/// Render a whole run: the greeting, every diagnostic, and the count.
pub fn report(source: &SourceFile, diags: &[Diagnostic]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{GREETING}");

    for diag in diags {
        out.push('\n');
        out.push_str(&diagnostic(source, diag));
    }

    out.push('\n');
    let _ = writeln!(out, "{}.", count_of(diags.len()));
    out
}

/// `1 error` or `12 errors`, so the last line reads as a sentence either way.
fn count_of(n: usize) -> String {
    if n == 1 { "1 error".to_string() } else { format!("{n} errors") }
}

/// A field that may hold nothing, one thing, or several. One goes on the same line as its
/// label; several are listed under it, so a long list stays readable.
fn write_field(out: &mut String, label: &str, values: &[String]) {
    match values {
        [] => {}
        [only] => {
            let _ = writeln!(out, "{label}: {only}");
        }
        many => {
            let _ = writeln!(out, "{label}:");
            for value in many {
                let _ = writeln!(out, "  - {value}");
            }
        }
    }
}

/// The source lines an error points at, each with its carets underneath.
fn snippet(source: &SourceFile, diag: &Diagnostic) -> String {
    let mut labels: Vec<_> = diag.labels.iter().collect();
    labels.sort_by_key(|l| l.span.start);

    // Wide enough for the largest line number, so the gutters line up.
    let widest = labels
        .iter()
        .map(|l| source.line_of(l.span.start).to_string().len())
        .max()
        .unwrap_or(1);

    let mut out = String::new();
    for label in labels {
        let line = source.line_of(label.span.start);
        let (indent, under) = source.caret_layout(label.span);
        let mark = match label.style {
            LabelStyle::Primary => '^',
            LabelStyle::Secondary => '~',
        };

        let _ = writeln!(out, "{:>widest$} | {}", line, source.line_text(line), widest = widest + 2);
        let _ = writeln!(
            out,
            "{:>widest$} | {}{} {}",
            "",
            " ".repeat(indent),
            mark.to_string().repeat(under),
            label.note,
            widest = widest + 2
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Diagnostic;
    use crate::source::Span;

    const PROGRAM: &str = "var.local.ui32 ['total'] = [|0|];\n\
                           \n\
                           loop.temp.range.ui8 ['i'] = [|1|, |10|] {\n\
                           set ['total'] = [|55|];\n\
                           }\n";

    fn source() -> SourceFile {
        SourceFile::new("src/main.lr", PROGRAM)
    }

    /// The worked example from the README, built the way the compiler will build it.
    fn cannot_change() -> Diagnostic {
        // Point at `local` in the chain, which is where the missing `mut` would go —
        // rather than at the name, which is not the part that is wrong.
        let declared = PROGRAM.find("local").unwrap();
        let changed = PROGRAM.rfind("['total']").unwrap() + 1;
        Diagnostic::new(
            "E0104",
            "`'total'` cannot be changed, because its declaration never said it could.",
        )
        .secondary(
            Span::new(declared, declared + "local".len()),
            "declared here, and `mut` is not in the chain",
        )
        .primary(Span::new(changed, changed + 7), "changed here")
        .rule("a variable changes only if its declaration says `mut`")
        .tip("`mut` goes between the visibility and the type.")
        .fix("line 1 — `var.local.mut.ui32 ['total'] = [|0|];`")
    }

    #[test]
    fn one_error_reads_the_way_the_readme_says() {
        let out = report(&source(), &[cannot_change()]);
        let expected = "\
Hello, I think there may be thing(s) wrong with your code. I'm sorry, if I'm wrong.

file: src/main.lr, line: 4, column: 6 (src/main.lr:4:6)

`'total'` cannot be changed, because its declaration never said it could.

  1 | var.local.ui32 ['total'] = [|0|];
    |     ~~~~~ declared here, and `mut` is not in the chain
  4 | set ['total'] = [|55|];
    |      ^^^^^^^ changed here

Error code: E0104
Rule(s) broken: a variable changes only if its declaration says `mut`
Tip(s): `mut` goes between the visibility and the type.
Suggested fix(s): line 1 — `var.local.mut.ui32 ['total'] = [|0|];`

1 error.
";
        assert_eq!(out, expected, "\n--- got ---\n{out}");
    }

    #[test]
    fn the_apology_is_made_once_however_many_errors_follow() {
        let out = report(&source(), &[cannot_change(), cannot_change(), cannot_change()]);
        assert_eq!(out.matches(GREETING).count(), 1);
        assert_eq!(out.matches("Error code: E0104").count(), 3);
        assert!(out.ends_with("3 errors.\n"));
    }

    #[test]
    fn a_clean_run_still_counts() {
        let out = report(&source(), &[]);
        assert!(out.starts_with(GREETING));
        assert!(out.ends_with("0 errors.\n"));
    }

    #[test]
    fn several_rules_are_listed_rather_than_run_together() {
        let diag = Diagnostic::new("E0001", "two things at once.")
            .primary(Span::new(0, 3), "here")
            .rule("the first rule")
            .rule("the second rule");
        let out = diagnostic(&source(), &diag);
        assert!(out.contains("Rule(s) broken:\n  - the first rule\n  - the second rule\n"), "{out}");
    }

    #[test]
    fn empty_fields_are_left_out_entirely() {
        let diag =
            Diagnostic::new("E0002", "nothing more to say.").primary(Span::new(0, 3), "here");
        let out = diagnostic(&source(), &diag);
        assert!(out.contains("Error code: E0002"));
        assert!(!out.contains("Tip(s)"));
        assert!(!out.contains("Suggested fix(s)"));
        assert!(!out.contains("Rule(s)"));
    }

    #[test]
    fn a_caret_under_an_emoji_name_is_two_cells_wide() {
        // The whole reason the layout is measured in cells: a caret counted in characters
        // would be one column short here, and would sit under the wrong quote.
        let family = "🧑‍🧑‍🧒‍🧒";
        let text = format!("var.local.b16 ['{family}'] = [|1|];\n");
        let src = SourceFile::new("src/main.lr", text.clone());
        let at = text.find(family).unwrap();

        let diag = Diagnostic::new("E0003", "that name is not declared.")
            .primary(Span::new(at, at + family.len()), "used here");
        let out = diagnostic(&src, &diag);

        assert!(out.contains("column: 17 (src/main.lr:1:17)"), "{out}");
        assert!(out.contains("\n    |                 ^^ used here\n"), "{out}");

        // Sixteen spaces of indent, then two carets, which is where the emoji is drawn.
        let caret_line = out.lines().find(|l| l.contains('^')).unwrap();
        let body = caret_line.split_once("| ").unwrap().1;
        assert_eq!(body.len() - body.trim_start().len(), 16);
    }

    #[test]
    fn the_gutter_widens_for_larger_line_numbers() {
        let mut text = String::new();
        for _ in 0..120 {
            text.push_str("print[\"x\" \\n];\n");
        }
        let src = SourceFile::new("src/main.lr", text.clone());
        let late = text.len() - 15;
        let diag =
            Diagnostic::new("E0004", "something here.").primary(Span::new(late, late + 5), "here");
        let out = diagnostic(&src, &diag);
        assert!(out.contains("  120 | "), "{out}");
    }
}
