use oxc_allocator::Allocator;
use oxc_ast::ast::Statement;
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tempfile::TempDir;
use tokio::process::Command;

const KERNEL: &str = include_str!("../../../kernels/typescript/kernel.ts");
const RUN: &str = include_str!("../../../kernels/typescript/run.ts");
const RUNTIME: &str = include_str!("../../../runtimes/typescript/codingpad/index.ts");

pub(crate) fn command() -> Result<(Command, Arc<TempDir>), String> {
    let files = tempfile::tempdir().map_err(|error| error.to_string())?;
    let kernel = files.path().join("kernel.ts");
    std::fs::write(&kernel, KERNEL).map_err(|error| error.to_string())?;
    std::fs::write(files.path().join("runtime.ts"), RUNTIME).map_err(|error| error.to_string())?;
    std::fs::write(files.path().join("run.ts"), RUN).map_err(|error| error.to_string())?;
    let mut command = Command::new(find_bun());
    command
        .current_dir(files.path())
        .arg("--no-env-file")
        .arg(kernel);
    Ok((command, Arc::new(files)))
}

fn find_bun() -> PathBuf {
    let name = if cfg!(windows) { "bun.exe" } else { "bun" };
    let path = std::env::var_os("PATH").unwrap_or_default();
    let install = std::env::var_os("BUN_INSTALL").filter(|path| !path.is_empty());
    std::env::split_paths(&path)
        .chain(install.map(|path| PathBuf::from(path).join("bin")))
        .chain(dirs::home_dir().map(|home| home.join(".bun/bin")))
        .find_map(|directory| bun_in_directory(&directory, name, cfg!(windows)))
        .and_then(|path| std::path::absolute(path).ok())
        .unwrap_or_else(|| PathBuf::from(name))
}

fn bun_in_directory(directory: &Path, name: &str, windows: bool) -> Option<PathBuf> {
    let executable = directory.join(name);
    if executable.is_file() {
        return Some(executable);
    }

    // npm creates bun.cmd on Windows. Start the executable it points to
    // directly so the supervisor owns the Bun process, including on Close.
    if windows && directory.join("bun.cmd").is_file() {
        let packages = if directory.ends_with(".bin") {
            directory.parent()?.to_owned()
        } else {
            directory.join("node_modules")
        };
        let executable = packages.join("bun/bin/bun.exe");
        return executable.is_file().then_some(executable);
    }
    None
}

#[derive(Serialize)]
pub(crate) struct FinalExpression {
    start: usize,
    end: usize,
    append: bool,
}

// Bun compiles the script itself. Parse only to find the last expression's
// byte range, leaving declarations, imports, and syntax diagnostics to Bun.
pub(crate) fn final_expression(source: &str) -> Option<FinalExpression> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
    if !parsed.errors.is_empty() {
        return None;
    }
    let (span, append) = match parsed.program.body.last() {
        Some(Statement::ExpressionStatement(statement)) => (statement.expression.span(), false),
        None => (parsed.program.directives.last()?.expression.span(), true),
        _ => return None,
    };
    // Oxc reports UTF-8 byte offsets. Bun slices JavaScript strings in UTF-16.
    let start = source[..span.start as usize].encode_utf16().count();
    let length = span.source_text(source).encode_utf16().count();
    Some(FinalExpression {
        start,
        end: start + length,
        append,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_executable_behind_the_windows_npm_shim() {
        let directory = tempfile::tempdir().unwrap();
        let packages = directory.path().join("node_modules");
        let binary = packages.join("bun/bin/bun.exe");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::File::create(&binary).unwrap();

        let local = packages.join(".bin");
        std::fs::create_dir(&local).unwrap();
        for shim_directory in [directory.path(), &local] {
            std::fs::File::create(shim_directory.join("bun.cmd")).unwrap();
            assert_eq!(
                bun_in_directory(shim_directory, "bun.exe", true),
                Some(binary.clone())
            );
        }
    }
}
