//! Bundled compiler/kernel assets; only a .NET runtime is needed by installed apps.
use std::{path::PathBuf, sync::Arc};
use tokio::{process::Command, sync::OnceCell};

use crate::toolchains::{newest_runtime_version, resolve_dotnet};

include!(concat!(env!("OUT_DIR"), "/csharp_assets.rs"));

pub(crate) const NOT_FOUND: &str = ".NET was not found.";

pub(super) struct Installation {
    executable: PathBuf,
    version: String,
    files: Arc<tempfile::TempDir>,
}

// One successful preparation per supervisor (the desktop's app lifetime).
// Failed or cancelled preparation remains retryable, including missing .NET.
pub(super) async fn command(
    cache: &OnceCell<Installation>,
    data_dir: Option<&std::path::Path>,
    dotnet: Option<&std::path::Path>,
) -> Result<(Command, Arc<tempfile::TempDir>), String> {
    let installation = cache
        .get_or_try_init(|| async {
            let executable = match dotnet {
                Some(dotnet) => dotnet.to_path_buf(),
                None => resolve_dotnet(data_dir).await,
            };
            prepare(executable).await
        })
        .await?;
    let mut command = Command::new(&installation.executable);
    command
        .args(["exec", "--fx-version", &installation.version])
        .arg(installation.files.path().join("CodingPad.Kernel.dll"));
    Ok((command, installation.files.clone()))
}

async fn prepare(executable: PathBuf) -> Result<Installation, String> {
    let mut probe = Command::new(&executable);
    probe.arg("--list-runtimes").kill_on_drop(true);
    #[cfg(windows)]
    probe.creation_flags(0x08000000);
    let output = probe
        .output()
        .await
        .map_err(|error| format!("{NOT_FOUND} Install the .NET SDK and run again. {error}"))?;
    let version = newest_runtime_version(&String::from_utf8_lossy(&output.stdout)).ok_or(
        format!("{NOT_FOUND} No supported .NET runtime was found. Install the .NET 10 SDK or later and run again."),
    )?;
    let files = tempfile::tempdir().map_err(|e| e.to_string())?;
    for (name, data) in ASSETS {
        std::fs::write(files.path().join(name), data).map_err(|e| e.to_string())?;
    }
    Ok(Installation {
        executable,
        version,
        files: Arc::new(files),
    })
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn concurrent_launches_share_one_preparation_and_keep_assets_alive() {
        let cache = tokio::sync::OnceCell::new();
        let (first, second) = tokio::join!(
            super::command(&cache, None, None),
            super::command(&cache, None, None)
        );
        let (first_command, first_files) = first.unwrap();
        let (second_command, second_files) = second.unwrap();
        assert!(std::sync::Arc::ptr_eq(&first_files, &second_files));
        assert_eq!(
            first_command.as_std().get_program(),
            second_command.as_std().get_program()
        );
        assert_eq!(
            first_command.as_std().get_args().collect::<Vec<_>>(),
            second_command.as_std().get_args().collect::<Vec<_>>()
        );
        let directory = first_files.path().to_path_buf();
        drop(cache);
        assert!(directory.join("CodingPad.Kernel.dll").is_file());
        drop(first_files);
        drop(second_files);
        assert!(!directory.exists());
    }
}
