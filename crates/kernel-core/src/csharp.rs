//! Bundled compiler/kernel assets; only a .NET runtime is needed by installed apps.
use std::path::PathBuf;
use tokio::process::Command;

include!(concat!(env!("OUT_DIR"), "/csharp_assets.rs"));

fn find_dotnet() -> PathBuf {
    let name = if cfg!(windows) {
        "dotnet.exe"
    } else {
        "dotnet"
    };
    let mut directories: Vec<_> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    for variable in ["DOTNET_ROOT", "DOTNET_ROOT_X64", "DOTNET_ROOT_ARM64"] {
        if let Some(path) = std::env::var_os(variable) {
            directories.push(path.into());
        }
    }
    if let Some(home) = dirs::home_dir() {
        directories.push(home.join(".dotnet"));
    }
    directories.extend(
        [
            "/usr/local/share/dotnet",
            "/usr/share/dotnet",
            "/usr/lib/dotnet",
        ]
        .map(PathBuf::from),
    );
    #[cfg(windows)]
    if let Some(programs) = std::env::var_os("ProgramFiles") {
        directories.push(PathBuf::from(programs).join("dotnet"));
    }
    directories
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| name.into())
}

pub(super) async fn command() -> Result<(Command, tempfile::TempDir), String> {
    let executable = find_dotnet();
    let mut probe = Command::new(&executable);
    probe.arg("--list-runtimes").kill_on_drop(true);
    #[cfg(windows)]
    probe.creation_flags(0x08000000);
    let output = probe.output().await.map_err(|error| {
        format!(".NET 10 or later was not found. Install the .NET SDK and run again. {error}")
    })?;
    let version = newest_runtime(&String::from_utf8_lossy(&output.stdout)).ok_or(
        "No supported .NET runtime was found. Install the .NET 10 SDK or later and run again.",
    )?;
    let files = tempfile::tempdir().map_err(|e| e.to_string())?;
    for (name, data) in ASSETS {
        std::fs::write(files.path().join(name), data).map_err(|e| e.to_string())?;
    }
    let mut command = Command::new(executable);
    command
        .args(["exec", "--fx-version", &version])
        .arg(files.path().join("CodingPad.Kernel.dll"));
    Ok((command, files))
}

fn newest_runtime(output: &str) -> Option<String> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            if parts.next()? != "Microsoft.NETCore.App" {
                return None;
            }
            let version = parts.next()?;
            let numbers: Vec<u32> = version
                .split('.')
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?;
            if numbers.len() != 3 || numbers[0] < 10 {
                return None;
            }
            Some((numbers, version.to_owned()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, version)| version)
}

#[cfg(test)]
mod tests {
    #[test]
    fn newest_supported_runtime() {
        assert_eq!(super::newest_runtime("Microsoft.NETCore.App 9.0.9 [/a]\nMicrosoft.NETCore.App 10.0.9 [/b]\nMicrosoft.NETCore.App 10.0.11 [/b]\nMicrosoft.NETCore.App 11.0.0 [/b]\nMicrosoft.NETCore.App 12.0.0-preview.1 [/b]\nMicrosoft.AspNetCore.App 12.0.0 [/b]"), Some("11.0.0".into()));
        assert_eq!(
            super::newest_runtime("Microsoft.NETCore.App 9.0.9 [/a]"),
            None
        );
    }
}
