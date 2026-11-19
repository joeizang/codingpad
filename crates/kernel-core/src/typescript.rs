use oxc_allocator::Allocator;
use oxc_ast::ast::Statement;
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};
use serde::Serialize;
use std::{
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::{Child, ChildStdin, Command},
    time::timeout,
};

const KERNEL: &str = include_str!("../../../kernels/typescript/kernel.ts");
const RUN: &str = include_str!("../../../kernels/typescript/run.ts");
const DUMP: &str = include_str!("../../../kernels/typescript/dump.ts");
const ERRORS: &str = include_str!("../../../kernels/typescript/errors.ts");
const RUNTIME: &str = include_str!("../../../runtimes/typescript/codingpad/index.ts");

pub(crate) fn command() -> Result<(Command, Arc<TempDir>), String> {
    let files = tempfile::tempdir().map_err(|error| error.to_string())?;
    let kernel = files.path().join("kernel.ts");
    std::fs::write(&kernel, KERNEL).map_err(|error| error.to_string())?;
    std::fs::write(files.path().join("runtime.ts"), RUNTIME).map_err(|error| error.to_string())?;
    std::fs::write(files.path().join("run.ts"), RUN).map_err(|error| error.to_string())?;
    std::fs::write(files.path().join("dump.ts"), DUMP).map_err(|error| error.to_string())?;
    std::fs::write(files.path().join("errors.ts"), ERRORS).map_err(|error| error.to_string())?;
    let scratch = files.path().join("scratch");
    std::fs::create_dir(&scratch).map_err(|error| error.to_string())?;
    let mut command = Command::new(find_bun());
    command
        .current_dir(scratch)
        .arg("--no-env-file")
        .arg(kernel);
    Ok((command, Arc::new(files)))
}

pub(crate) async fn connect(
    child: &mut Child,
    stdin: &mut ChildStdin,
) -> Result<TcpStream, String> {
    // Standard output belongs to the worker's native file descriptors as well
    // as the kernel. Use an authenticated loopback connection for replies.
    let connect = async {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let mut token = [0u8; 32];
        getrandom::fill(&mut token).map_err(std::io::Error::other)?;
        let hello = serde_json::json!({"port": listener.local_addr()?.port(), "token": token});
        stdin.write_all(format!("{hello}\n").as_bytes()).await?;
        stdin.flush().await?;

        tokio::select! {
            stream = accept_kernel(&listener, &token) => stream,
            status = child.wait() => Err(std::io::Error::other(format!(
                "Bun exited during startup ({})", status?
            ))),
        }
    };

    timeout(Duration::from_secs(5), connect)
        .await
        .map_err(|_| "Timed out connecting to the Bun kernel.".to_owned())?
        .map_err(|error| format!("Could not connect to the Bun kernel: {error}"))
}

async fn accept_kernel(listener: &TcpListener, token: &[u8; 32]) -> std::io::Result<TcpStream> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let mut received = [0u8; 32];
        if matches!(
            timeout(Duration::from_millis(250), stream.read_exact(&mut received)).await,
            Ok(Ok(_))
        ) && &received == token
        {
            return Ok(stream);
        }
    }
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
