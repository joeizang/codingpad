use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    for path in [
        "kernels/csharp/Program.cs",
        "kernels/csharp/CodingPad.Kernel.csproj",
        "kernels/csharp/packages.lock.json",
        "runtimes/dotnet/CodingPad/CodingPad.csproj",
        "runtimes/dotnet/CodingPad/DumpExtensions.cs",
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
    let mut entries = Vec::new();
    for file in fs::read_dir(publish).unwrap() {
        let file = file.unwrap();
        if file.file_type().unwrap().is_file() {
            entries.push(format!(
                "({:?}, include_bytes!({:?})),",
                file.file_name().to_str().unwrap(),
                file.path()
            ));
        }
    }
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
