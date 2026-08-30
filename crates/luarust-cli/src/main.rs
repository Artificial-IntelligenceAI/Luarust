//! The `luarust` command.

use luarust_diag::{Diagnostic, SourceFile};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
luarust — a language that would rather not guess

    luarust run <file.lr>      check it, then run it
    luarust check <file.lr>    check it and stop
    luarust --help             this
";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    let path = args.next().map(PathBuf::from);

    match (command.as_deref(), path) {
        (Some("run"), Some(path)) => act(path, true),
        (Some("check"), Some(path)) => act(path, false),
        (Some("--help" | "-h" | "help"), _) => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        (Some(command), None) => {
            eprintln!("`{command}` needs a file to work on.\n\n{USAGE}");
            ExitCode::from(2)
        }
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn act(path: PathBuf, then_run: bool) -> ExitCode {
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(why) => {
            eprintln!("{} could not be read: {why}", path.display());
            return ExitCode::from(2);
        }
    };
    let source = SourceFile::new(path, text);

    let mut errors: Vec<Diagnostic> = Vec::new();

    // Each stage runs only if the one before it had nothing to say, because a parser fed
    // broken tokens invents problems that are not really there.
    let lexed = luarust_lex::lex(source.text());
    errors.extend(lexed.errors);

    let mut checked = None;
    if errors.is_empty() {
        let parsed = luarust_parse::parse(source.text(), &lexed.tokens);
        errors.extend(parsed.errors);
        if errors.is_empty() {
            let (program, problems) = luarust_check::check(&parsed.program);
            errors.extend(problems);
            checked = Some(program);
        }
    }

    if !errors.is_empty() {
        eprint!("{}", luarust_diag::report(&source, &errors));
        return ExitCode::FAILURE;
    }

    let Some(program) = checked else { return ExitCode::FAILURE };
    if !then_run {
        return ExitCode::SUCCESS;
    }

    let mut out = std::io::stdout().lock();
    match luarust_interp::run(&program, &mut out) {
        Ok(()) => {
            let _ = out.flush();
            ExitCode::SUCCESS
        }
        Err(stopped) => {
            let _ = out.flush();
            eprint!("{}", luarust_diag::report(&source, &[stopped.diagnostic()]));
            ExitCode::FAILURE
        }
    }
}
