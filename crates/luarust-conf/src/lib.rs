//! `Luarust.toml` — what a project has decided, once, for every file in it.
//!
//! A setting written at the top of a file still wins for that file. This is only the
//! answer for the files that do not say.
//!
//! The reader below understands the part of TOML this file needs and refuses the rest by
//! name rather than by guessing. That is deliberate: a full TOML library is a large
//! dependency to take on for four lines of settings, and a project that will not put a
//! garbage collector on a device it is not needed on should not put a deserialiser in the
//! toolchain it does not need either.

use luarust_core::value::Overflow;
use luarust_diag::{Diagnostic, Span};

/// What the project file said, with anything it did not mention left at its default.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Project {
    /// `[defaults] overflow`
    pub overflow: Overflow,
    /// `[defaults] no-visibility-stated`
    pub visibility_required: bool,
    /// `[build] embed-source` — whether a chunk carries the text it was built from.
    pub embed_source: bool,
    /// `[build] decimal-encoding` — which of IEEE 754's two ways of writing a decimal
    /// significand a chunk uses. They hold the same numbers, so nothing about arithmetic
    /// depends on it; it decides the bit pattern that gets written out.
    pub dpd: bool,
    /// `[gc] mode` — whether a running program collects its dead arrays, and how eagerly.
    pub gc: Collect,
    /// `[defaults] float-printing` — how much of a binary float a program writes out.
    pub floats: Floats,
}

/// How much of a binary float a program writes out.
///
/// `0.1` is not representable in binary, so a `b64` holds the nearest value it has. Both
/// answers below are true about that value and they disagree about what to say:
///
/// ```text
///   exact      0.1000000000000000055511151231257827021181583404541015625
///   shortest   0.1
/// ```
///
/// Exact hides nothing, which is the same argument that put `er` in the language. Shortest
/// is what most languages print and is far easier to read. Neither is wrong, so it is a
/// setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Floats {
    /// The value that is held, whole.
    Exact,
    /// The fewest digits that name this number and no other, at its own format.
    Shortest,
}

/// What a program does about arrays nothing can reach any more.
///
/// This is a footprint decision as much as a speed one. A program that says `"off"` has
/// no collector in it at all, which is the rule the whole project runs on: a program pays
/// for what it uses and not for what somebody else might.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Collect {
    /// Never. The heap only grows, which is exactly right for a program that makes a few
    /// arrays and exits, and exactly wrong for one that loops.
    Off,
    /// When enough has been handed out to be worth the walk. The ordinary answer.
    Silent,
    /// At every opportunity. Slower, and it holds the smallest heap a program can run in.
    Aggressive,
}

impl Collect {
    /// How many bytes may be handed out before a collection, or `None` for never.
    pub fn threshold(self) -> Option<usize> {
        match self {
            Collect::Off => None,
            // A megabyte is enough that a small program never collects at all and a
            // looping one collects rarely.
            Collect::Silent => Some(1 << 20),
            // Not zero: collecting after an array of nothing would walk the roots for no
            // reason. One page is small enough to feel immediate.
            Collect::Aggressive => Some(4096),
        }
    }
}

impl Default for Project {
    fn default() -> Self {
        // An integer wraps, a declaration that states no visibility is restricted rather
        // than refused, and a chunk carries its source. Each of those is the answer that
        // surprises a beginner least, and each can be turned around.
        Project {
            overflow: Overflow::Wrap,
            visibility_required: false,
            embed_source: true,
            dpd: false,
            // Off, because a program that never makes an array has nothing to collect and
            // should not carry a collector. Saying `"silent"` is what turns it on.
            gc: Collect::Off,
            // Exact, because a program that would rather not guess should not print `0.1`
            // for a number that is not one tenth.
            floats: Floats::Exact,
        }
    }
}

/// The name looked for, walking up from the file being compiled.
pub const FILENAME: &str = "Luarust.toml";

/// Find the project file for a source file, if the project has one.
///
/// Every directory from the file's own up to the root is looked in, so a project is
/// whatever has a `Luarust.toml` at the top of it and nothing has to be declared.
pub fn find(from: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut at = from.parent()?.to_path_buf();
    loop {
        let candidate = at.join(FILENAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        if !at.pop() {
            return None;
        }
    }
}

/// Read a project file's text.
///
/// Anything it gets wrong is reported the way a mistake in a source file would be, since
/// a settings file is code that decides how code is built.
pub fn read(text: &str) -> (Project, Vec<Diagnostic>) {
    let mut project = Project::default();
    let mut errors = Vec::new();
    let mut section = String::new();
    let mut at = 0usize;

    for line in text.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let body = strip_comment(line);
        let trimmed = body.trim();
        if trimmed.is_empty() {
            continue;
        }

        // A table header. Nothing here nests, so the name is taken whole.
        if let Some(name) = trimmed.strip_prefix('[').and_then(|n| n.strip_suffix(']')) {
            let name = name.trim();
            if !matches!(name, "defaults" | "build" | "gc") {
                errors.push(
                    Diagnostic::new("C0001", format!("there is no `[{name}]` section."))
                        .primary(locate(body, start, trimmed), "written here")
                        .rule("a project file has the sections Luarust knows")
                        .tip("the sections are `[defaults]` and `[build]`.")
                        .fix("delete it, or correct the name."),
                );
                section.clear();
                continue;
            }
            section = name.to_string();
            continue;
        }

        let Some((key, value)) = trimmed.split_once('=') else {
            errors.push(
                Diagnostic::new("C0002", "this is not a setting.".to_string())
                    .primary(locate(body, start, trimmed), "written here")
                    .rule("every line of a project file is a section or `key = value`")
                    .tip("a comment starts with `#`.")
                    .fix("write it as `key = value`, or delete the line."),
            );
            continue;
        };
        let key = key.trim();
        let raw = value.trim();
        let span = locate(body, start, raw);

        if section.is_empty() {
            errors.push(
                Diagnostic::new("C0003", format!("`{key}` is not under any section."))
                    .primary(locate(body, start, trimmed), "written here")
                    .rule("a setting belongs to the section above it")
                    .tip("`overflow` and `no-visibility-stated` are `[defaults]`; `embed-source` and `decimal-encoding` are `[build]`; `mode` is `[gc]`.")
                    .fix("put a section header above it."),
            );
            continue;
        }

        match (section.as_str(), key) {
            ("defaults", "overflow") => match unquote(raw) {
                Some("wrap") => project.overflow = Overflow::Wrap,
                Some("trap") => project.overflow = Overflow::Trap,
                _ => errors.push(bad_value(key, raw, span, "`\"wrap\"` or `\"trap\"`")),
            },
            ("defaults", "no-visibility-stated") => match unquote(raw) {
                Some("restricted") => project.visibility_required = false,
                Some("error") => project.visibility_required = true,
                _ => errors.push(bad_value(key, raw, span, "`\"restricted\"` or `\"error\"`")),
            },
            ("build", "embed-source") => match raw {
                "true" => project.embed_source = true,
                "false" => project.embed_source = false,
                _ => errors.push(bad_value(key, raw, span, "`true` or `false`")),
            },
            ("build", "decimal-encoding") => match unquote(raw) {
                Some("bid") => project.dpd = false,
                Some("dpd") => project.dpd = true,
                _ => errors.push(bad_value(key, raw, span, "`\"bid\"` or `\"dpd\"`")),
            },
            ("defaults", "float-printing") => match unquote(raw) {
                Some("exact") => project.floats = Floats::Exact,
                Some("shortest") => project.floats = Floats::Shortest,
                _ => errors.push(bad_value(key, raw, span, "`\"exact\"` or `\"shortest\"`")),
            },
            ("gc", "mode") => match unquote(raw) {
                Some("off") => project.gc = Collect::Off,
                Some("silent") => project.gc = Collect::Silent,
                Some("aggressive") => project.gc = Collect::Aggressive,
                _ => errors.push(bad_value(
                    key,
                    raw,
                    span,
                    "`\"off\"`, `\"silent\"` or `\"aggressive\"`",
                )),
            },
            _ => errors.push(
                Diagnostic::new("C0004", format!("`[{section}]` has no `{key}` setting."))
                    .primary(locate(body, start, key), "written here")
                    .rule("a project file sets only settings that exist")
                    .tip(match section.as_str() {
                        "defaults" => "`[defaults]` has `overflow`, `no-visibility-stated` and `float-printing`.",
                        "gc" => "`[gc]` has `mode`.",
                        _ => "`[build]` has `embed-source` and `decimal-encoding`.",
                    })
                    .fix("delete it, or correct the spelling."),
            ),
        }
    }

    (project, errors)
}

fn bad_value(key: &str, raw: &str, span: Span, allowed: &str) -> Diagnostic {
    Diagnostic::new("C0005", format!("`{raw}` is not something `{key}` can be set to."))
        .primary(span, "written here")
        .rule("a setting is given one of the values it allows")
        .tip(format!("`{key}` may be {allowed}."))
        .fix(format!("write {allowed}."))
}

/// Where a piece of a line is, counted from the start of the whole file.
fn locate(line: &str, line_start: usize, piece: &str) -> Span {
    let offset = find_offset(line, piece);
    Span::new(line_start + offset, line_start + offset + piece.len())
}

fn find_offset(haystack: &str, needle: &str) -> usize {
    haystack.find(needle).unwrap_or(0)
}

/// Everything before a `#`, which starts a comment unless it is inside quotes.
fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    for (at, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..at],
            _ => {}
        }
    }
    line
}

/// The inside of a `"…"`, or nothing if it was not one.
fn unquote(raw: &str) -> Option<&str> {
    raw.strip_prefix('"')?.strip_suffix('"')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(text: &str) -> Project {
        let (project, errors) = read(text);
        assert!(errors.is_empty(), "expected no errors, got {errors:#?}");
        project
    }

    fn codes(text: &str) -> Vec<String> {
        read(text).1.into_iter().map(|e| e.code).collect()
    }

    #[test]
    fn an_empty_file_is_every_default() {
        assert_eq!(clean(""), Project::default());
        assert_eq!(clean("\n\n# nothing but a comment\n"), Project::default());
    }

    #[test]
    fn the_readme_project_file_reads_the_way_it_looks() {
        let project = clean("[defaults]\nno-visibility-stated = \"error\"\noverflow = \"trap\"\n");
        assert!(project.visibility_required);
        assert_eq!(project.overflow, Overflow::Trap);
        // And what it did not mention is untouched.
        assert!(project.embed_source);
    }

    #[test]
    fn the_decimal_encoding_can_be_chosen() {
        assert!(!clean("[build]\ndecimal-encoding = \"bid\"\n").dpd);
        assert!(clean("[build]\ndecimal-encoding = \"dpd\"\n").dpd);
    }

    #[test]
    fn how_much_of_a_float_to_print_is_a_choice() {
        assert_eq!(Project::default().floats, Floats::Exact, "exact unless asked otherwise");
        assert_eq!(clean("[defaults]\nfloat-printing = \"exact\"\n").floats, Floats::Exact);
        assert_eq!(clean("[defaults]\nfloat-printing = \"shortest\"\n").floats, Floats::Shortest);

        let (_, errors) = read("[defaults]\nfloat-printing = \"some\"\n");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "C0005");
    }

    #[test]
    fn a_hash_is_a_comment_wherever_it_is() {
        // TOML's comment character. The reader has always handled it; the CLI used to lex
        // a project file as if it were a program, and `#` is the first thing in TOML that
        // is nothing at all in Luarust, so that is where it gave itself away.
        let project = clean(
            "# what this project wants\n\
             [defaults]\n\
             overflow = \"trap\"   # stop rather than wrap\n\
             \n\
             # collecting\n\
             [gc]\n\
             mode = \"silent\"\n",
        );
        assert_eq!(project.overflow, Overflow::Trap);
        assert_eq!(project.gc, Collect::Silent);

        // And inside quotes it is just a character.
        let (_, errors) = read("[gc]\nmode = \"#\"\n");
        assert_eq!(errors.len(), 1, "`\"#\"` is a bad value, not a comment");
        assert_eq!(errors[0].code, "C0005");
    }

    #[test]
    fn collecting_is_asked_for_and_never_assumed() {
        assert_eq!(Project::default().gc, Collect::Off, "a program collects only if it says so");
        assert_eq!(clean("[gc]\nmode = \"off\"\n").gc, Collect::Off);
        assert_eq!(clean("[gc]\nmode = \"silent\"\n").gc, Collect::Silent);
        assert_eq!(clean("[gc]\nmode = \"aggressive\"\n").gc, Collect::Aggressive);

        assert_eq!(Collect::Off.threshold(), None, "off means no collector at all");
        assert!(Collect::Aggressive.threshold() < Collect::Silent.threshold());

        let (_, errors) = read("[gc]\nmode = \"sometimes\"\n");
        assert_eq!(errors.len(), 1, "a mode that does not exist is refused");
        assert_eq!(errors[0].code, "C0005");
        // BID by default, because it is what the arithmetic works in anyway.
        assert!(!Project::default().dpd);
        assert_eq!(codes("[build]\ndecimal-encoding = \"packed\"\n"), ["C0005"]);
    }

    #[test]
    fn a_chunk_can_be_told_not_to_carry_its_source() {
        assert!(!clean("[build]\nembed-source = false\n").embed_source);
        assert!(clean("[build]\nembed-source = true\n").embed_source);
    }

    #[test]
    fn a_comment_ends_a_line_but_not_inside_quotes() {
        assert_eq!(clean("[defaults]\noverflow = \"trap\" # louder\n").overflow, Overflow::Trap);
        assert_eq!(strip_comment("a = \"#trap\"\n"), "a = \"#trap\"\n");
    }

    #[test]
    fn every_way_of_getting_it_wrong_is_named() {
        assert_eq!(codes("[defualts]\n"), ["C0001"]);
        assert_eq!(codes("[defaults]\njust a sentence\n"), ["C0002"]);
        assert_eq!(codes("overflow = \"trap\"\n"), ["C0003"]);
        assert_eq!(codes("[defaults]\noverfloww = \"trap\"\n"), ["C0004"]);
        assert_eq!(codes("[defaults]\noverflow = \"explode\"\n"), ["C0005"]);
        assert_eq!(codes("[build]\nembed-source = \"false\"\n"), ["C0005"]);
        // A quoted `true` is not a boolean, and the message says which it wanted.
        assert_eq!(codes("[defaults]\noverflow = wrap\n"), ["C0005"]);
    }

    #[test]
    fn an_error_points_at_the_part_that_was_wrong() {
        let text = "[defaults]\noverflow = \"explode\"\n";
        let errors = read(text).1;
        let span = errors[0].primary_label().expect("the value is pointed at").span;
        assert_eq!(&text[span.start..span.end], "\"explode\"");
    }

    #[test]
    fn the_last_word_on_a_setting_is_the_last_one_written() {
        assert_eq!(clean("[defaults]\noverflow = \"trap\"\noverflow = \"wrap\"\n").overflow, Overflow::Wrap);
    }
}
