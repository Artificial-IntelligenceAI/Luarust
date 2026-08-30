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
            if !matches!(name, "defaults" | "build") {
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
                    .tip("`overflow` and `no-visibility-stated` are `[defaults]`; `embed-source` and `decimal-encoding` are `[build]`.")
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
            _ => errors.push(
                Diagnostic::new("C0004", format!("`[{section}]` has no `{key}` setting."))
                    .primary(locate(body, start, key), "written here")
                    .rule("a project file sets only settings that exist")
                    .tip(match section.as_str() {
                        "defaults" => "`[defaults]` has `overflow` and `no-visibility-stated`.",
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
