//! Reusable download/verification and atomic binary installation for toolchains.
//! URLs and digests come from the Rust release manifest, never from the webview.
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use tempfile::NamedTempFile;

pub struct Artifact<'a> {
    pub url: &'a str,
    pub sha256: &'a str,
    pub max_bytes: u64,
}

/// Download over HTTPS, enforcing a size limit and a pinned SHA-256 before use.
/// Kept separate from extraction so future SDK installers can reuse it.
pub async fn download_verified(
    artifact: &Artifact<'_>,
    directory: &Path,
) -> Result<NamedTempFile, String> {
    if !artifact.url.starts_with("https://") {
        return Err("Tool downloads require HTTPS".into());
    }
    fs::create_dir_all(directory).map_err(|e| format!("Could not create the tool folder: {e}"))?;
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = client
        .get(artifact.url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| format!("Tool download failed. Check your connection and retry: {e}"))?;
    let mut file = NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut total = 0;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("Tool download interrupted; retry: {e}"))?
    {
        total += chunk.len() as u64;
        if total > artifact.max_bytes {
            return Err("Tool download exceeded its size limit".into());
        }
        hash.update(&chunk);
        file.write_all(&chunk)
            .map_err(|e| format!("Could not save the tool download: {e}"))?;
    }
    verify_hash(&format!("{:x}", hash.finalize()), artifact.sha256)?;
    file.flush().map_err(|e| e.to_string())?;
    Ok(file)
}

fn verify_hash(actual: &str, expected: &str) -> Result<(), String> {
    if actual != expected {
        return Err(
            "Tool download failed SHA-256 verification. Nothing was installed; retry the download."
                .into(),
        );
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub enum Archive {
    TarGz,
    Zip,
}

// Extract only the expected regular file. No archive paths, links, permissions,
// or other entries are ever applied to the destination filesystem.
fn extract_binary(
    archive: &Path,
    format: Archive,
    member: &str,
    directory: &Path,
) -> Result<NamedTempFile, String> {
    let mut output = tempfile::Builder::new()
        .suffix(if cfg!(windows) { ".exe" } else { ".bin" })
        .tempfile_in(directory)
        .map_err(|e| e.to_string())?;
    match format {
        Archive::TarGz => {
            let input = fs::File::open(archive).map_err(|e| e.to_string())?;
            let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(input));
            let mut found = false;
            for entry in tar.entries().map_err(|e| e.to_string())? {
                let mut entry = entry.map_err(|e| e.to_string())?;
                if entry.path().map_err(|e| e.to_string())? == Path::new(member) {
                    if !entry.header().entry_type().is_file() {
                        return Err("Tool archive member is not a regular file".into());
                    }
                    copy_binary(&mut entry, &mut output)?;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err("Tool archive is missing its executable".into());
            }
        }
        Archive::Zip => {
            let input = fs::File::open(archive).map_err(|e| e.to_string())?;
            let mut zip = zip::ZipArchive::new(input).map_err(|e| e.to_string())?;
            let mut entry = zip
                .by_name(member)
                .map_err(|e| format!("Tool archive is missing its executable: {e}"))?;
            if !entry.is_file()
                || entry
                    .unix_mode()
                    .is_some_and(|mode| mode & 0o170000 == 0o120000)
            {
                return Err("Tool archive member is not a regular file".into());
            }
            copy_binary(&mut entry, &mut output)?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        output
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    output.as_file().sync_all().map_err(|e| e.to_string())?;
    Ok(output)
}

fn copy_binary(input: &mut impl Read, output: &mut impl Write) -> Result<(), String> {
    const LIMIT: u64 = 128 * 1024 * 1024;
    let size = std::io::copy(&mut input.take(LIMIT + 1), output).map_err(|e| e.to_string())?;
    if size == 0 || size > LIMIT {
        return Err("Tool executable has an invalid size".into());
    }
    Ok(())
}

/// Install without overwriting a user tool or exposing a partially written file.
pub async fn install_binary(
    artifact: &Artifact<'_>,
    format: Archive,
    member: &str,
    destination: &Path,
) -> Result<PathBuf, String> {
    let directory = destination.parent().ok_or("Missing tool directory")?;
    let archive = download_verified(artifact, directory).await?;
    let member = member.to_owned();
    let directory = directory.to_owned();
    let binary = tokio::task::spawn_blocking(move || {
        extract_binary(archive.path(), format, &member, &directory)
    })
    .await
    .map_err(|e| e.to_string())??;
    // Close the writer before executing (Unix ETXTBSY), and use an .exe suffix on Windows.
    let probe = binary.into_temp_path();
    let mut command = tokio::process::Command::new(&probe);
    command.arg("--version").kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let result = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .map_err(|_| "Installed tool did not respond; retry installation".to_string())?
        .map_err(|e| format!("Could not run the downloaded tool: {e}"))?;
    if !result.status.success() {
        return Err("Downloaded tool failed its startup check".into());
    }
    match probe.persist_noclobber(destination) {
        Ok(_) => Ok(destination.to_owned()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(destination.to_owned())
        }
        Err(error) => Err(format!("Could not install the tool: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tar_file(path: &Path, member: &str, link: bool) {
        let encoder = flate2::write::GzEncoder::new(
            fs::File::create(path).unwrap(),
            flate2::Compression::default(),
        );
        let mut tar = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o755);
        if link {
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_link_name("/etc/passwd").unwrap();
            header.set_size(0);
        } else {
            header.set_size(4);
        }
        header.set_cksum();
        tar.append_data(
            &mut header,
            member,
            if link { &b""[..] } else { &b"tool"[..] },
        )
        .unwrap();
        tar.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn corrupted_downloads_are_rejected() {
        let digest = format!("{:x}", Sha256::digest(b"verified release"));
        assert!(verify_hash(&digest, &digest).is_ok());
        assert!(verify_hash(
            &format!("{:x}", Sha256::digest(b"corrupt download")),
            &digest
        )
        .unwrap_err()
        .contains("Nothing was installed"));
    }

    #[test]
    fn tar_extracts_only_the_expected_regular_file_and_cleans_staging() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("release.tar.gz");
        tar_file(&archive, "release/uv", false);
        let output =
            extract_binary(&archive, Archive::TarGz, "release/uv", directory.path()).unwrap();
        assert_eq!(fs::read(output.path()).unwrap(), b"tool");
        let staged = output.path().to_owned();
        drop(output);
        assert!(!staged.exists());
        assert!(!directory.path().join("release").exists());
        assert!(extract_binary(&archive, Archive::TarGz, "missing", directory.path()).is_err());
        tar_file(&archive, "release/uv", true);
        assert!(
            extract_binary(&archive, Archive::TarGz, "release/uv", directory.path())
                .unwrap_err()
                .contains("regular file")
        );
    }

    #[test]
    fn windows_zip_ignores_unrelated_paths_and_rejects_invalid_archives() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("release.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("../escaped", options).unwrap();
        zip.write_all(b"ignored").unwrap();
        zip.start_file("uv.exe", options).unwrap();
        zip.write_all(b"tool").unwrap();
        zip.finish().unwrap();
        let output = extract_binary(&archive, Archive::Zip, "uv.exe", directory.path()).unwrap();
        assert_eq!(fs::read(output.path()).unwrap(), b"tool");
        assert!(!directory.path().join("escaped").exists());
        fs::write(&archive, "truncated").unwrap();
        assert!(extract_binary(&archive, Archive::Zip, "uv.exe", directory.path()).is_err());
    }

    #[tokio::test]
    async fn insecure_download_urls_are_rejected_without_creating_files() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("absent");
        assert!(download_verified(
            &Artifact {
                url: "http://example.com/tool",
                sha256: "",
                max_bytes: 100
            },
            &destination
        )
        .await
        .is_err());
        assert!(!destination.exists());
    }
}
