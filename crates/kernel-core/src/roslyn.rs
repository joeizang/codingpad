//! Locate the Roslyn language server used for C# code intelligence.
//!
//! Roslyn ships its language server with the C# extension for VS Code rather
//! than as a general-purpose .NET tool. CodingPad uses an explicit override
//! first, then its own data folder, then a local C# extension install. A later
//! milestone can offer a one-click download.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
use tokio::process::Command;

const EXECUTABLE: &str = "Microsoft.CodeAnalysis.LanguageServer";

pub(crate) struct Roslyn {
    program: PathBuf,
    prefix: Vec<OsString>,
}

impl Roslyn {
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.prefix);
        command.args([
            "--stdio",
            "--autoLoadProjects",
            "--telemetryLevel",
            "off",
            "--logLevel",
            "Warning",
        ]);
        command
            .env("DOTNET_NOLOGO", "1")
            .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1");
        command
    }
}

/// Resolve the language server, preferring a path the user chose explicitly.
/// `dotnet` is the host resolved by the toolchain module so a private SDK can
/// run framework-dependent servers when no user SDK is installed.
pub(crate) fn find(data_dir: Option<&Path>, dotnet: &Path) -> Result<Roslyn, String> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("CODINGPAD_ROSLYN_LSP") {
        candidates.push(PathBuf::from(path));
    }
    if let Some(data_dir) = data_dir {
        candidates.extend(server_files(&data_dir.join("roslyn")));
    }
    candidates.extend(
        std::env::var_os("PATH")
            .map(|path| {
                std::env::split_paths(&path)
                    .flat_map(|directory| server_files(&directory))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    );
    if let Some(home) = dirs::home_dir() {
        for folder in [
            ".vscode/extensions",
            ".vscode-insiders/extensions",
            ".vscode-server/extensions",
            ".vscode-server-insiders/extensions",
            ".cursor/extensions",
        ] {
            let extensions = home.join(folder);
            let Ok(entries) = std::fs::read_dir(&extensions) else {
                continue;
            };
            let mut installed: Vec<_> = entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("ms-dotnettools.csharp"))
                })
                .collect();
            // Newest first. Compare versions numerically: as text,
            // "csharp-2.160.4" sorts after "csharp-11.9.41".
            installed.sort_by_key(|path| std::cmp::Reverse(extension_version(path)));
            for extension in installed {
                candidates.extend(server_files(&extension.join(".roslyn")));
            }
        }
    }
    for candidate in candidates {
        if let Some(server) = resolve(&candidate, dotnet) {
            return Ok(server);
        }
    }
    Err("The Roslyn language server was not found. Install the C# extension in VS Code, or set CODINGPAD_ROSLYN_LSP to Microsoft.CodeAnalysis.LanguageServer.".to_owned())
}

/// The version in a VS Code extension folder name such as
/// `ms-dotnettools.csharp-11.9.41-linux-x64`, as numbers for ordering.
fn extension_version(path: &Path) -> Vec<u64> {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("ms-dotnettools.csharp-"))
        .and_then(|rest| rest.split('-').next())
        .map(|version| {
            version
                .split('.')
                .map(|part| part.parse().unwrap_or(0))
                .collect()
        })
        .unwrap_or_default()
}

fn server_files(directory: &Path) -> Vec<PathBuf> {
    if directory
        .extension()
        .is_some_and(|extension| extension == "dll")
    {
        return vec![directory.to_path_buf()];
    }
    if directory.is_file() {
        return vec![directory.to_path_buf()];
    }
    vec![
        directory.join(EXECUTABLE),
        directory.join(format!("{EXECUTABLE}.exe")),
        directory.join(format!("{EXECUTABLE}.dll")),
    ]
}

fn resolve(candidate: &Path, dotnet: &Path) -> Option<Roslyn> {
    if !candidate.is_file() {
        return None;
    }
    if candidate
        .extension()
        .is_some_and(|extension| extension == "dll")
    {
        // A framework-dependent server needs the dotnet host; the runtime it
        // targets is the same one the resolved SDK provides.
        return Some(Roslyn {
            program: dotnet.to_path_buf(),
            prefix: vec!["exec".into(), candidate.as_os_str().to_owned()],
        });
    }
    Some(Roslyn {
        program: candidate.to_path_buf(),
        prefix: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_an_explicit_dll_and_wraps_it_with_dotnet() {
        let directory = tempfile::tempdir().unwrap();
        let dll = directory.path().join(format!("{EXECUTABLE}.dll"));
        std::fs::write(&dll, "").unwrap();
        let server = resolve(&dll, Path::new("dotnet")).unwrap();
        assert_eq!(server.program, PathBuf::from("dotnet"));
        assert_eq!(
            server.prefix,
            [OsString::from("exec"), dll.into_os_string()]
        );
        let command = server.command();
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"--stdio".to_owned()));
        assert!(args.contains(&"--autoLoadProjects".to_owned()));
    }

    #[test]
    fn extension_versions_order_numerically() {
        let mut folders = [
            "ms-dotnettools.csharp-2.160.4-linux-x64",
            "ms-dotnettools.csharp-11.9.41-linux-x64",
            "ms-dotnettools.csharp-11.10.2-win32-x64",
            "ms-dotnettools.csharp-unexpected",
        ]
        .map(PathBuf::from);
        folders.sort_by_key(|path| std::cmp::Reverse(extension_version(path)));
        assert_eq!(
            folders.map(|path| path.to_string_lossy().into_owned()),
            [
                "ms-dotnettools.csharp-11.10.2-win32-x64",
                "ms-dotnettools.csharp-11.9.41-linux-x64",
                "ms-dotnettools.csharp-2.160.4-linux-x64",
                "ms-dotnettools.csharp-unexpected",
            ]
        );
    }

    #[test]
    fn finds_a_server_in_the_private_data_folder() {
        let data = tempfile::tempdir().unwrap();
        let directory = data.path().join("roslyn");
        std::fs::create_dir_all(&directory).unwrap();
        let executable = directory.join(EXECUTABLE);
        std::fs::write(&executable, "").unwrap();
        let server = find(Some(data.path()), Path::new("dotnet")).unwrap();
        assert_eq!(server.program, executable);
    }
}
