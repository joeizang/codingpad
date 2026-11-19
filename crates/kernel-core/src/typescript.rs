use oxc_allocator::Allocator;
use oxc_ast::ast::Statement;
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};
use std::{path::PathBuf, sync::Arc};
use tempfile::TempDir;
use tokio::process::Command;

const KERNEL: &str = include_str!("../../../kernels/typescript/kernel.ts");
const RUNTIME: &str = include_str!("../../../runtimes/typescript/codingpad/index.ts");

pub(crate) fn command() -> Result<(Command, Arc<TempDir>), String> {
    let files = tempfile::tempdir().map_err(|error| error.to_string())?;
    let kernel = files.path().join("kernel.ts");
    std::fs::write(&kernel, KERNEL).map_err(|error| error.to_string())?;
    std::fs::write(files.path().join("runtime.ts"), RUNTIME).map_err(|error| error.to_string())?;
    let mut command = Command::new(find_bun());
    command.args(["--no-install", "--no-env-file"]).arg(kernel);
    Ok((command, Arc::new(files)))
}

fn find_bun() -> PathBuf {
    let name = if cfg!(windows) { "bun.exe" } else { "bun" };
    let path = std::env::var_os("PATH").unwrap_or_default();
    let install = std::env::var_os("BUN_INSTALL").filter(|path| !path.is_empty());
    std::env::split_paths(&path)
        .chain(install.map(|path| PathBuf::from(path).join("bin")))
        .chain(dirs::home_dir().map(|home| home.join(".bun/bin")))
        .chain(
            cfg!(target_os = "macos")
                .then_some(["/opt/homebrew/bin", "/usr/local/bin"])
                .into_iter()
                .flatten()
                .map(PathBuf::from),
        )
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

// Bun compiles the script itself. Parse only to find the last expression's
// byte range, leaving declarations, imports, and syntax diagnostics to Bun.
pub(crate) fn instrument(source: &str) -> std::borrow::Cow<'_, str> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
    if !parsed.errors.is_empty() {
        return source.into();
    }
    let span = match parsed.program.body.last() {
        Some(Statement::ExpressionStatement(statement)) => statement.expression.span(),
        // A string literal in the directive prologue is still an expression.
        // Keep the directive in place and dump its value separately.
        None if !parsed.program.directives.is_empty() => {
            let literal = parsed.program.directives.last().unwrap().expression.span();
            return format!(
                "{source}\n;(await import(\"codingpad:internal\")).dumpFinal({});",
                literal.source_text(source)
            )
            .into();
        }
        _ => return source.into(),
    };
    let start = span.start as usize;
    let end = span.end as usize;
    format!(
        "{};(await import(\"codingpad:internal\")).dumpFinal(({})){}",
        &source[..start],
        &source[start..end],
        &source[end..]
    )
    .into()
}
