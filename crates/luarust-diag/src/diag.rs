//! What a Luarust error knows about itself.
//!
//! Everything here is data. Turning it into the message a person reads is
//! [`crate::render`]'s job, so that no part of the compiler has to know how an error is
//! laid out in order to report one.

use crate::source::Span;

/// One place in the source that an error wants to point at, and why.
#[derive(Clone, Debug)]
pub struct Label {
    pub span: Span,
    /// What is true about this spot. Shown beside the carets.
    pub note: String,
    pub style: LabelStyle,
}

/// Whether a label is the thing that went wrong, or context for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LabelStyle {
    /// The error itself. Underlined with `^`, and its position is the one reported at the
    /// top of the message.
    Primary,
    /// Something that explains it — a declaration, an earlier use. Underlined with `~`.
    Secondary,
}

impl Label {
    pub fn primary(span: Span, note: impl Into<String>) -> Self {
        Self { span, note: note.into(), style: LabelStyle::Primary }
    }

    pub fn secondary(span: Span, note: impl Into<String>) -> Self {
        Self { span, note: note.into(), style: LabelStyle::Secondary }
    }
}

/// One thing wrong with a program.
///
/// A diagnostic is required to carry a rule and at least one primary label. The rule is
/// what separates a Luarust error from an apology: the message says what went wrong here,
/// and the rule says what is true everywhere.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    /// The stable code, `E0104` and so on, for looking a thing up.
    pub code: String,
    /// One sentence saying what is wrong. Not a category — a sentence.
    pub message: String,
    /// The rule or rules broken. What is true everywhere, not just here.
    pub rules: Vec<String>,
    /// Things worth knowing that are not the fix.
    pub tips: Vec<String>,
    /// What to actually do about it.
    pub fixes: Vec<String>,
    /// Where to look, in the order the reader should look.
    pub labels: Vec<Label>,
}

impl Diagnostic {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            rules: Vec::new(),
            tips: Vec::new(),
            fixes: Vec::new(),
            labels: Vec::new(),
        }
    }

    pub fn rule(mut self, rule: impl Into<String>) -> Self {
        self.rules.push(rule.into());
        self
    }

    pub fn tip(mut self, tip: impl Into<String>) -> Self {
        self.tips.push(tip.into());
        self
    }

    pub fn fix(mut self, fix: impl Into<String>) -> Self {
        self.fixes.push(fix.into());
        self
    }

    pub fn label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    pub fn primary(self, span: Span, note: impl Into<String>) -> Self {
        self.label(Label::primary(span, note))
    }

    pub fn secondary(self, span: Span, note: impl Into<String>) -> Self {
        self.label(Label::secondary(span, note))
    }

    /// The label the message is about — the one whose position is reported at the top.
    pub fn primary_label(&self) -> Option<&Label> {
        self.labels
            .iter()
            .find(|l| l.style == LabelStyle::Primary)
            .or_else(|| self.labels.first())
    }
}
