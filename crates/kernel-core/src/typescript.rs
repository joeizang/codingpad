use crate::shadow::ShadowProject;
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, CallExpression, Expression, ImportExpression, Statement, TSExternalModuleReference,
    TSImportType, TemplateLiteral,
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
const CODINGPAD_TYPES: &str = include_str!("../../../runtimes/typescript/codingpad/index.d.ts");
const FALLBACK_GLOBALS: &str = include_str!("../../../runtimes/typescript/codingpad/globals.d.ts");

/// Pinned with the Bun version CI installs and the private installer uses.
const BUN_TYPES_VERSION: &str = crate::toolchains::BUN_VERSION;

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
    pub(crate) shadow: ShadowProject,
    pub(crate) imports: Imports,
}

impl Project {
    pub(crate) async fn prepare(
        bun: &Path,
        imports: Imports,
        types: bool,
        mut notice: impl FnMut(String),
        #[cfg(test)] registry: Option<&str>,
    ) -> Result<Self, String> {
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
        fs::write(root.join("package.json"), manifest.to_string()).map_err(|error| {
            format!("Could not write package.json for the TypeScript shadow project: {error}")
        })?;
        fs::write(root.join("codingpad.d.ts"), CODINGPAD_TYPES).map_err(|error| {
            format!("Could not write codingpad.d.ts for the TypeScript shadow project: {error}")
        })?;
        fs::write(root.join("globals.d.ts"), FALLBACK_GLOBALS).map_err(|error| {
            format!("Could not write globals.d.ts for the TypeScript shadow project: {error}")
        })?;
        if !imports.0.is_empty() {
            let packages = imports.0.keys().cloned().collect::<Vec<_>>().join(", ");
            notice(format!("Installing TypeScript packages: {packages}"));
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
            #[cfg(test)]
            if let Some(registry) = registry {
                command
                    .arg("--registry")
                    .arg(registry)
                    .arg("--cache-dir")
                    .arg(root.join("test-cache"));
            }
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
                let details = String::from_utf8_lossy(&output.stderr);
                return Err(format!(
                    "Could not install TypeScript packages ({packages}). Check the package versions and registry connection. Bun exited with {}.\n{}",
                    output.status,
                    details.trim()
                ));
            }
        }
        // Prefer real Bun/Node types when the editor asks for them. Run does
        // not need types and must not pay a package install before the kernel
        // starts. The checked-in globals keep common built-ins available when
        // the editor reuses a project that Run prepared, or when installation
        // fails.
        #[cfg(test)]
        let bun_types = types && registry.is_none() && install_bun_types(bun, root).await;
        #[cfg(not(test))]
        let bun_types = types && install_bun_types(bun, root).await;
        let mut files = vec!["script.ts", "codingpad.d.ts"];
        if !bun_types {
            files.push("globals.d.ts");
        }
        let tsconfig = json!({
            "compilerOptions": {
                "module": "ESNext",
                "moduleResolution": "Bundler",
                "target": "ESNext",
                "noEmit": true,
                "strict": true,
                "skipLibCheck": true,
                "lib": ["ESNext", "DOM", "DOM.Iterable"],
                "types": if bun_types { json!(["bun"]) } else { json!([]) },
                // TypeScript 7 removed baseUrl; paths resolve beside tsconfig.
                "paths": { "codingpad": ["./codingpad.d.ts"] },
            },
            "files": files,
        });
        fs::write(root.join("tsconfig.json"), tsconfig.to_string()).map_err(|error| {
            format!("Could not write tsconfig.json for the TypeScript shadow project: {error}")
        })?;
        Ok(Self { shadow, imports })
    }

    pub(crate) fn stage(&self, source: &str) -> Result<(), String> {
        self.shadow.write_source(source)
    }
}

async fn install_bun_types(bun: &Path, root: &Path) -> bool {
    let mut command = Command::new(bun);
    command
        .current_dir(root)
        .args([
            "--no-env-file",
            "add",
            "--dev",
            "--prefer-offline",
            "--ignore-scripts",
            "--no-progress",
        ])
        .arg(format!("@types/bun@{BUN_TYPES_VERSION}"))
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x08000000 | 0x00000200); // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
    let installed = timeout(Duration::from_secs(20), command.status())
        .await
        .ok()
        .and_then(Result::ok)
        .is_some_and(|status| status.success());
    installed && root.join("node_modules/@types/bun/package.json").is_file()
}

pub(crate) fn bun_not_found(error: &io::Error) -> Option<String> {
    (error.kind() == io::ErrorKind::NotFound).then(|| {
        "Bun was not found. Choose Install Bun and run to install a private copy for CodingPad, or install Bun from https://bun.com/docs/installation and run the script again.".into()
    })
}

const KERNEL_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

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

    // Bun exiting is caught above, so this limit only bounds a hung startup.
    // A cold start on a loaded Windows machine, with antivirus scanning the new
    // kernel files, can take well over 5 s.
    timeout(KERNEL_STARTUP_TIMEOUT, connect)
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
        match &import.source {
            Expression::StringLiteral(source) => self.0.push(source.value.as_str()),
            Expression::TemplateLiteral(template) => self.template(template),
            _ => (),
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

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if matches!(&call.callee, Expression::Identifier(name) if name.name == "require") {
            match call.arguments.as_slice() {
                [Argument::StringLiteral(source)] => self.0.push(source.value.as_str()),
                [Argument::TemplateLiteral(template)] => self.template(template),
                _ => (),
            }
        }
        walk::walk_call_expression(self, call);
    }
}

impl<'a> Specifiers<'a> {
    fn template(&mut self, template: &TemplateLiteral<'a>) {
        if let [quasi] = template.quasis.as_slice() {
            self.0
                .extend(quasi.value.cooked.as_ref().map(|text| text.as_str()));
        }
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
        || version.ends_with('.')
        || !version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-+^~".contains(&byte))
    {
        return Err(format!(
            "Invalid TypeScript package import \"{specifier}\". Use a portable npm version such as 3, 3.x, ^3.20.0, ~3.20.0 or next."
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
    use crate::{lsp::LanguageServers, Language, Supervisor};
    use tokio::io::{AsyncBufReadExt, BufReader};

    async fn fixture_registry() -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let registry = format!("http://{}", listener.local_addr().unwrap());
        let manifest = json!({
            "name": "codingpad-fixture",
            "version": "1.2.3",
            "exports": {
                ".": {"types": "./index.d.ts", "default": "./index.cjs"},
            },
        });
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        for (path, contents) in [
            ("package/package.json", manifest.to_string()),
            ("package/index.cjs", "exports.answer = 42;".into()),
            (
                "package/index.d.ts",
                "export declare const answer: number;".into(),
            ),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(contents.len() as u64);
            header.set_cksum();
            tar.append_data(&mut header, path, contents.as_bytes())
                .unwrap();
        }
        let archive = tar.into_inner().unwrap().finish().unwrap();
        let mut version = manifest;
        version["dist"] = json!({"tarball": format!("{registry}/fixture.tgz")});
        let metadata = json!({
            "name": "codingpad-fixture",
            "dist-tags": {"latest": "1.2.3"},
            "versions": {"1.2.3": version},
        })
        .to_string();
        let server = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let mut reader = BufReader::new(stream);
                let mut request = String::new();
                reader.read_line(&mut request).await.unwrap();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).await.unwrap() == 0 || header == "\r\n" {
                        break;
                    }
                }
                let (status, mime, body) = match request.split_whitespace().nth(1) {
                    Some("/codingpad-fixture") => {
                        ("200 OK", "application/json", metadata.as_bytes())
                    }
                    Some("/fixture.tgz") => {
                        ("200 OK", "application/octet-stream", archive.as_slice())
                    }
                    _ => ("404 Not Found", "text/plain", &b"Unknown package"[..]),
                };
                let stream = reader.get_mut();
                let headers = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                if stream.write_all(headers.as_bytes()).await.is_ok() {
                    let _ = stream.write_all(body).await;
                }
            }
        });
        (registry, server)
    }

    #[tokio::test]
    async fn shares_versioned_packages_between_the_editor_and_kernel_without_a_live_registry() {
        let (registry, server) = fixture_registry().await;
        let mut supervisor = Supervisor::default();
        Arc::get_mut(&mut supervisor.shadow_projects)
            .unwrap()
            .registry = Some(registry);
        let editor = LanguageServers::default().with_shadow_projects(supervisor.shadow_projects());
        editor.begin_session("editor").await;
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("packages.ts");
        let source = r#"
const { answer } = require("codingpad-fixture@1");
const { answer: imported } = await import(`codingpad-fixture@1`);
answer + imported
"#;
        fs::write(&script, "throw new Error('saved copy ran')").unwrap();

        let missing = "import 'codingpad-fixture@999';";
        let mut notices = Vec::new();
        let error = timeout(
            Duration::from_secs(20),
            supervisor.run_language_at(
                "packages",
                missing,
                Language::Typescript,
                script.to_str(),
                |event| notices.push(event.params["text"].clone()),
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        if error.starts_with("Bun was not found.") && env::var_os("CI").is_none() {
            server.abort();
            return;
        }
        assert!(
            error.contains("Could not install TypeScript packages (codingpad-fixture@999)"),
            "{error}"
        );
        assert_eq!(
            notices,
            ["Installing TypeScript packages: codingpad-fixture@999\n"]
        );

        let prepared = timeout(
            Duration::from_secs(20),
            editor.prepare_typescript("packages", source, "editor"),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        let document = url::Url::parse(&prepared.uri)
            .unwrap()
            .to_file_path()
            .unwrap();
        assert_eq!(fs::read_to_string(&document).unwrap(), source);
        assert!(document
            .with_file_name("node_modules")
            .join("codingpad-fixture@1/index.d.ts")
            .is_file());
        server.abort();
        let _ = server.await;

        for expected in [84, 84, 85] {
            let source = if expected == 85 {
                format!("{source} + 1")
            } else {
                source.to_owned()
            };
            let mut events = Vec::new();
            // Leave room for the kernel's own startup limit on slow runners.
            let result = timeout(
                KERNEL_STARTUP_TIMEOUT + Duration::from_secs(15),
                supervisor.run_language_at(
                    "packages",
                    &source,
                    Language::Typescript,
                    script.to_str(),
                    |event| events.push(event),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(result.ok, "{events:?}");
            assert_eq!(events[0].params["node"]["v"], expected, "{events:?}");
            assert_eq!(fs::read_to_string(&document).unwrap(), source);
            supervisor.reset("packages").await.unwrap();
        }
        supervisor.close("packages").await;
        assert!(!document.exists());
        assert_eq!(
            fs::read_to_string(script).unwrap(),
            "throw new Error('saved copy ran')"
        );
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn rejects_nonportable_version_ranges_on_every_platform() {
        for version in ["*", ">=3 <4", "3.x || 4.x"] {
            let source = format!("import 'zod@{version}';");
            assert!(parse(&source)
                .err()
                .unwrap()
                .contains("portable npm version"));
        }
    }

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

    #[tokio::test]
    async fn tsconfig_points_codingpad_at_the_helper_declaration() {
        let project = Project::prepare(
            &find_bun(),
            Imports::default(),
            false,
            |_| {},
            Some("http://127.0.0.1:9"),
        )
        .await
        .unwrap();
        let root = project.shadow.root();
        let config: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(root.join("tsconfig.json")).unwrap()).unwrap();
        assert_eq!(
            config["compilerOptions"]["paths"]["codingpad"][0],
            "./codingpad.d.ts"
        );
        assert_eq!(config["compilerOptions"]["strict"], true);
        assert_eq!(config["compilerOptions"]["types"], json!([]));
        assert!(config["files"]
            .as_array()
            .unwrap()
            .contains(&json!("globals.d.ts")));
        assert!(root.join("codingpad.d.ts").is_file());
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
