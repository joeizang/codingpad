use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    for path in [
        "kernels/csharp/Program.cs",
        "kernels/csharp/DumpSession.cs",
        "kernels/csharp/ScriptDirectives.cs",
        "kernels/csharp/ScriptDependencies.cs",
        "kernels/csharp/CodingPad.Kernel.csproj",
        "kernels/csharp/packages.lock.json",
        "runtimes/dotnet/CodingPad/CodingPad.csproj",
        "runtimes/dotnet/CodingPad/DumpExtensions.cs",
        "runtimes/dotnet/CodingPad/DatabaseClient.cs",
        "runtimes/dotnet/CodingPad/RichValues.cs",
        "runtimes/dotnet/CodingPad/ScriptHelpers.cs",
    ] {
        println!("cargo:rerun-if-changed=../../{path}");
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let publish = out.join("csharp");
    let status = Command::new("dotnet")
        .args([
            "publish",
            "../../kernels/csharp/CodingPad.Kernel.csproj",
            "--configuration",
            "Release",
            "-p:RestoreLockedMode=true",
            "--output",
        ])
        .arg(&publish)
        .status()
        .expect("Building CodingPad requires the .NET 10 SDK on PATH");
    assert!(status.success(), "Could not publish the bundled C# kernel");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target_arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "x86",
        "arm" => "arm",
        other => panic!("Unsupported .NET target architecture: {other}"),
    };
    let os = match target_os.as_str() {
        "macos" => "osx",
        "windows" => "win",
        "linux" if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("musl") => "linux-musl",
        "linux" => "linux",
        other => panic!("Unsupported .NET target OS: {other}"),
    };
    let rid = format!("{os}-{target_arch}");
    fn collect(
        root: &std::path::Path,
        dir: &std::path::Path,
        entries: &mut Vec<String>,
        rid: &str,
        os: &str,
    ) {
        for file in fs::read_dir(dir).unwrap() {
            let file = file.unwrap();
            if file.file_type().unwrap().is_dir() {
                if dir == root.join("runtimes") {
                    let name = file.file_name();
                    let name = name.to_string_lossy();
                    if name != rid
                        && !(name == "unix" && os != "win")
                        && !(name == "win" && os == "win")
                    {
                        continue;
                    }
                }
                collect(root, &file.path(), entries, rid, os);
            } else if file.file_type().unwrap().is_file() {
                entries.push(format!(
                    "({:?}, include_bytes!({:?})),",
                    file.path().strip_prefix(root).unwrap().to_string_lossy(),
                    file.path()
                ));
            }
        }
    }
    let mut entries = Vec::new();
    collect(&publish, &publish, &mut entries, &rid, os);
    entries.sort();
    fs::write(
        out.join("csharp_assets.rs"),
        format!(
            "const ASSETS: &[(&str, &[u8])] = &[{}];",
            entries.join("\n")
        ),
    )
    .unwrap();
}
