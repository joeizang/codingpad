use crate::shadow::ShadowProject;
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Expression, ImportExpression, Statement, TSExternalModuleReference, TSImportType,
};
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};
use serde::Serialize;
use serde_json::json;
use std::{
    collections::BTreeMap,
    env, fs, io,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::Stdio,
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

pub(crate) fn command(bun: &Path, project: &Project) -> Result<(Command, Arc<TempDir>), String> {
    let files = tempfile::tempdir().map_err(|error| error.to_string())?;
    let kernel = files.path().join("kernel.ts");
    std::fs::write(&kernel, KERNEL).map_err(|error| error.to_string())?;
    std::fs::write(files.path().join("runtime.ts"), RUNTIME).map_err(|error| error.to_string())?;
    std::fs::write(files.path().join("run.ts"), RUN).map_err(|error| error.to_string())?;
    std::fs::write(files.path().join("dump.ts"), DUMP).map_err(|error| error.to_string())?;
    std::fs::write(files.path().join("errors.ts"), ERRORS).map_err(|error| error.to_string())?;
    let scratch = files.path().join("scratch");
    std::fs::create_dir(&scratch).map_err(|error| error.to_string())?;
    let mut command = Command::new(bun);
    command
        .current_dir(scratch)
        .arg("--no-env-file")
        .arg(kernel);
    if !project.imports.0.is_empty() {
        // Bun's worker resolver can load installed packages from NODE_PATH
        // even when the editor snapshot lives outside the shadow project.
        let inherited = env::var_os("NODE_PATH").unwrap_or_default();
        let paths = std::iter::once(project.shadow.root().join("node_modules"))
            .chain(env::split_paths(&inherited).filter(|path| !path.as_os_str().is_empty()));
        command.env(
            "NODE_PATH",
            env::join_paths(paths).map_err(|error| {
                format!("Could not set the TypeScript package search path: {error}")
            })?,
        );
    }
    Ok((command, Arc::new(files)))
}

pub(crate) struct Project {
    shadow: ShadowProject,
    pub(crate) imports: Imports,
}

impl Project {
    pub(crate) async fn prepare(bun: &Path, imports: Imports) -> Result<Self, String> {
        let shadow = ShadowProject::temporary("script.ts")?;
        let root = shadow.root();
        // Install under the original module name, including its version.
        // TypeScript then resolves package exports and declarations normally.
        let manifest = json!({
            "name": "codingpad-script",
            "private": true,
            "type": "module",
            "dependencies": imports.0,
        });
        let tsconfig = json!({
            "compilerOptions": {
                "module": "ESNext",
                "moduleResolution": "Bundler",
                "target": "ESNext",
                "noEmit": true,
            },
            "files": ["script.ts"],
        });
        for (file, contents) in [("package.json", manifest), ("tsconfig.json", tsconfig)] {
            fs::write(root.join(file), contents.to_string()).map_err(|error| {
                format!("Could not write {file} for the TypeScript shadow project: {error}")
            })?;
        }
        if !imports.0.is_empty() {
            let mut command = Command::new(bun);
            command
                .current_dir(root)
                .args([
                    "--no-env-file",
                    "install",
                    "--prefer-offline",
                    "--ignore-scripts",
                    "--no-progress",
                ])
                .env("NO_COLOR", "1")
                .stdin(Stdio::null())
                .kill_on_drop(true);
            #[cfg(unix)]
            command.process_group(0);
            #[cfg(windows)]
            command.creation_flags(0x08000000 | 0x00000200); // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
            let output = command.output().await.map_err(|error| {
                bun_not_found(&error).unwrap_or_else(|| {
                    format!("Could not start Bun to install TypeScript packages: {error}")
                })
            })?;
            if !output.status.success() {
                let packages = imports.0.keys().cloned().collect::<Vec<_>>().join(", ");
                let details = String::from_utf8_lossy(&output.stderr);
                return Err(format!(
                    "Could not install TypeScript packages ({packages}). Check the package versions and registry connection. Bun exited with {}.\n{}",
                    output.status,
                    details.trim()
                ));
            }
        }
        Ok(Self { shadow, imports })
    }

    pub(crate) fn stage(&self, source: &str) -> Result<(), String> {
        self.shadow.write_source(source)
    }
}

pub(crate) fn bun_not_found(error: &io::Error) -> Option<String> {
    (error.kind() == io::ErrorKind::NotFound).then(|| {
        "Bun was not found. Install Bun from https://bun.com/docs/installation and run the script again.".into()
    })
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

pub(crate) fn find_bun() -> PathBuf {
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

pub(crate) struct Script {
    pub(crate) imports: Imports,
    pub(crate) final_expression: Option<FinalExpression>,
}

// Reuse the syntax tree for package discovery and the final-expression range.
// Bun still compiles the editor snapshot and reports its syntax errors.
pub(crate) fn parse(source: &str) -> Result<Option<Script>, String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
    if !parsed.errors.is_empty() {
        return Ok(None);
    }
    let mut imports = Imports::default();
    let mut specifiers = Specifiers::default();
    specifiers.0.extend(
        parsed
            .module_record
            .requested_modules
            .keys()
            .map(|name| name.as_str()),
    );
    specifiers.visit_program(&parsed.program);
    for specifier in specifiers.0 {
        if let Some((alias, target)) = versioned_package(specifier)? {
            imports.0.insert(alias.to_owned(), target);
        }
    }
    Ok(Some(Script {
        imports,
        final_expression: final_expression(source, &parsed.program),
    }))
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Imports(BTreeMap<String, String>);

#[derive(Default)]
struct Specifiers<'a>(Vec<&'a str>);

impl<'a> Visit<'a> for Specifiers<'a> {
    fn visit_import_expression(&mut self, import: &ImportExpression<'a>) {
        if let Expression::StringLiteral(source) = &import.source {
            self.0.push(source.value.as_str());
        }
        walk::walk_import_expression(self, import);
    }

    fn visit_ts_import_type(&mut self, import: &TSImportType<'a>) {
        self.0.push(import.source.value.as_str());
        walk::walk_ts_import_type(self, import);
    }

    fn visit_ts_external_module_reference(&mut self, import: &TSExternalModuleReference<'a>) {
        self.0.push(import.expression.value.as_str());
    }
}

fn versioned_package(specifier: &str) -> Result<Option<(&str, String)>, String> {
    // An initial @ belongs to the scope. A later @ before the package
    // subpath separates the npm name from its version.
    let start = if specifier.starts_with('@') {
        let Some((scope, _)) = specifier.split_once('/') else {
            return Ok(None);
        };
        scope.len() + 1
    } else {
        0
    };
    let package = specifier[start..].split('/').next().unwrap_or_default();
    let Some((name, version)) = package.split_once('@') else {
        return Ok(None);
    };
    let alias = &specifier[..start + package.len()];
    let npm_name = &specifier[..start + name.len()];
    let valid_name = |part: &str| {
        !part.is_empty()
            && !part.starts_with(['.', '-'])
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    };
    if !valid_name(name) || (start != 0 && !valid_name(&specifier[1..start - 1])) {
        return Ok(None);
    }
    if version.is_empty()
        || version.trim() != version
        || !version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-+^~*<>=| ".contains(&byte))
    {
        return Err(format!(
            "Invalid TypeScript package import \"{specifier}\". Use an npm version, range or tag after @."
        ));
    }
    if cfg!(windows) && version.contains(['*', '<', '>', '|']) {
        return Err(format!(
            "TypeScript package import \"{specifier}\" uses characters Windows cannot mirror. Use an exact version, tag, caret range or tilde range."
        ));
    }
    Ok(Some((alias, format!("npm:{npm_name}@{version}"))))
}

fn final_expression(source: &str, program: &oxc_ast::ast::Program<'_>) -> Option<FinalExpression> {
    let (span, append) = match program.body.last() {
        Some(Statement::ExpressionStatement(statement)) => (statement.expression.span(), false),
        None => (program.directives.last()?.expression.span(), true),
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
    fn mirrors_versioned_imports_and_exports_by_package() {
        let source = r#"
import { z } from "zod@3";
import type { ZodType } from "zod@3/v3";
export { default } from "@scope/pkg@^1.2.3/subpath";
export type { Item } from "exported-types@next";
type Value = import("imported-types@2").Value;
const dynamic = await import("zod@4");
const fromFile = await import("./files/data@2");
const untouched = "import \"string@1\"";
// import "comment@1";
import fs from "node:fs";
import "@scope/unversioned";
"#;
        let script = parse(source).unwrap().unwrap();
        assert_eq!(
            script.imports.0,
            BTreeMap::from([
                ("@scope/pkg@^1.2.3".into(), "npm:@scope/pkg@^1.2.3".into()),
                (
                    "exported-types@next".into(),
                    "npm:exported-types@next".into()
                ),
                ("imported-types@2".into(), "npm:imported-types@2".into()),
                ("zod@3".into(), "npm:zod@3".into()),
                ("zod@4".into(), "npm:zod@4".into()),
            ])
        );
    }

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
