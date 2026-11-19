//! Shadow project storage shared by language-specific project writers.
use crate::data_context::GeneratedContext;
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

use crate::lsp::path_to_uri;

const RUNTIME_SOURCES: &[(&str, &str)] = &[
    (
        "DumpExtensions.cs",
        include_str!("../../../runtimes/dotnet/CodingPad/DumpExtensions.cs"),
    ),
    (
        "RichValues.cs",
        include_str!("../../../runtimes/dotnet/CodingPad/RichValues.cs"),
    ),
    (
        "ScriptHelpers.cs",
        include_str!("../../../runtimes/dotnet/CodingPad/ScriptHelpers.cs"),
    ),
];

/// The parsed subset of `#:` directives that changes a shadow project.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Directives {
    packages: BTreeMap<String, String>,
    properties: BTreeMap<String, String>,
    web: bool,
}

impl Directives {
    pub(crate) fn parse(source: &str) -> Result<Self, String> {
        let mut result = Self::default();
        let mut seen = HashSet::new();
        let mut in_block_comment = false;
        for (index, line) in source.lines().enumerate() {
            let number = index + 1;
            let trimmed = line.trim();
            if in_block_comment {
                if trimmed.contains("*/") {
                    in_block_comment = false;
                }
                continue;
            }
            if trimmed.starts_with("/*") {
                in_block_comment = !trimmed.contains("*/");
                continue;
            }
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }
            let Some(directive) = trimmed.strip_prefix("#:") else {
                // File directives only live in leading trivia.
                break;
            };
            let fail = |message: &str| format!("line {number}: {message}");
            let Some(split) = directive.find([' ', '\t']) else {
                return Err(fail("Expected a directive name and value."));
            };
            let kind = &directive[..split];
            let value = directive[split..].trim();
            match kind {
                "project" => {
                    return Err(fail(
                        "#:project is not supported in CodingPad yet; runs with `dotnet run`.",
                    ))
                }
                "package" => {
                    let Some((name, version)) = value.split_once('@') else {
                        return Err(fail(
                            "Expected #:package Name@Version (NuGet version ranges and floating versions are supported).",
                        ));
                    };
                    let (name, version) = (name.trim(), version.trim());
                    if name.is_empty()
                        || version.is_empty()
                        || !name
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
                    {
                        return Err(fail(
                            "Expected #:package Name@Version (NuGet version ranges and floating versions are supported).",
                        ));
                    }
                    if !seen.insert(format!("package:{}", name.to_ascii_lowercase())) {
                        return Err(fail(&format!("Duplicate package directive: {name}")));
                    }
                    result.packages.insert(name.to_owned(), version.to_owned());
                }
                "sdk" => {
                    if !seen.insert("sdk".to_owned()) {
                        return Err(fail("Only one SDK directive is supported."));
                    }
                    result.web = match value {
                        "Microsoft.NET.Sdk" => false,
                        "Microsoft.NET.Sdk.Web" => true,
                        _ => {
                            return Err(fail(
                                "Supported SDKs: Microsoft.NET.Sdk and Microsoft.NET.Sdk.Web.",
                            ))
                        }
                    };
                }
                "property" => {
                    let Some((name, raw)) = value.split_once('=') else {
                        return Err(fail("Expected #:property Name=Value."));
                    };
                    let name = name.trim();
                    if name.is_empty() {
                        return Err(fail("Expected #:property Name=Value."));
                    }
                    if !seen.insert(format!("property:{}", name.to_ascii_lowercase())) {
                        return Err(fail(&format!("Duplicate property directive: {name}")));
                    }
                    let value = unquote(raw.trim()).map_err(|message| fail(&message))?;
                    validate_property(name, &value).map_err(|message| fail(&message))?;
                    let name = canonical_property(name).unwrap_or(name);
                    result.properties.insert(name.to_owned(), value);
                }
                other => return Err(fail(&format!("Unsupported file directive: #:{other}"))),
            }
        }
        Ok(result)
    }

    /// Add the packages a generated EF Core data context needs. User packages
    /// win when they already name the same package.
    pub(crate) fn add_data_packages(&mut self, kind: &str, source: &str) {
        for (name, version) in data_packages(kind, source) {
            self.packages
                .entry(name.to_owned())
                .or_insert_with(|| version.to_owned());
        }
    }

    /// A stable restart key for the language server.
    pub(crate) fn key(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    fn package_manifest(&self) -> String {
        self.packages
            .iter()
            .filter(|(name, _)| !name.eq_ignore_ascii_case("CodingPad"))
            .map(|(name, version)| {
                format!(
                    "    <PackageReference Include=\"{}\" Version=\"{}\" />",
                    escape(name),
                    escape(version)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn property_manifest(&self) -> String {
        let mut properties: BTreeMap<String, String> = BTreeMap::new();
        properties.insert("OutputType".to_owned(), "Exe".to_owned());
        properties.insert(
            "TargetFramework".to_owned(),
            self.properties
                .get("TargetFramework")
                .cloned()
                .unwrap_or_else(|| "net10.0".to_owned()),
        );
        properties.insert("Nullable".to_owned(), self.value("Nullable", "enable"));
        properties.insert(
            "ImplicitUsings".to_owned(),
            self.value("ImplicitUsings", "enable"),
        );
        properties.insert("WarningLevel".to_owned(), self.value("WarningLevel", "10"));
        properties.insert("EnableDefaultCompileItems".to_owned(), "false".to_owned());
        properties.insert("AssemblyName".to_owned(), "CodingPad.Script".to_owned());
        properties.insert("RollForward".to_owned(), "LatestMajor".to_owned());
        let no_warn = self.value("NoWarn", "");
        let no_warn = if no_warn.is_empty() {
            "CS5001".to_owned()
        } else {
            format!("CS5001;{no_warn}")
        };
        properties.insert("NoWarn".to_owned(), no_warn);
        properties.insert(
            "Features".to_owned(),
            "$(Features);FileBasedProgram".to_owned(),
        );
        for (name, value) in &self.properties {
            match name.as_str() {
                "TargetFramework" | "Nullable" | "ImplicitUsings" | "WarningLevel" | "NoWarn" => {}
                "CodingPadImplicitUsings" => {}
                "PublishAot" | "PackAsTool" => {}
                "LangVersion" if value == "default" => {}
                _ => {
                    properties.insert(name.clone(), value.clone());
                }
            }
        }
        properties
            .into_iter()
            .map(|(name, value)| {
                format!(
                    "    <{}>{}</{}>",
                    escape(&name),
                    escape(&value),
                    escape(&name)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn value(&self, name: &str, fallback: &str) -> String {
        self.properties
            .get(name)
            .cloned()
            .unwrap_or_else(|| fallback.to_owned())
    }

    fn flag(&self, name: &str, fallback: bool) -> bool {
        self.properties
            .get(name)
            .map(|value| value.eq_ignore_ascii_case("true"))
            .unwrap_or(fallback)
    }

    fn usings(&self) -> String {
        let mut usings = Vec::new();
        if self.flag("CodingPadImplicitUsings", true) {
            // Without SDK implicit usings the kernel still imports the common
            // namespaces through its helper block.
            if !self.implicit_usings_enabled() {
                usings.extend(
                    [
                        "System",
                        "System.Collections.Generic",
                        "System.Linq",
                        "System.Threading",
                        "System.Threading.Tasks",
                    ]
                    .map(|name| format!("global using {name};")),
                );
            }
            usings.push("global using CodingPad;".to_owned());
            usings.push("global using static CodingPad.ScriptHelpers;".to_owned());
        }
        usings.join("\n")
    }

    fn implicit_usings_enabled(&self) -> bool {
        matches!(
            self.value("ImplicitUsings", "enable").as_str(),
            "enable" | "true"
        )
    }
}

/// A generated project rooted beside a saved script (or in a temp folder).
pub(crate) struct ShadowProject {
    _temp: TempDir,
    root: PathBuf,
    parent: Option<PathBuf>,
    document: PathBuf,
    document_uri: String,
    root_uri: String,
}

impl ShadowProject {
    pub(crate) fn temporary(document_name: &str) -> Result<Self, String> {
        let temp = project_directory()?;
        let root = temp.path().to_path_buf();
        Self::new(temp, root, document_name, None)
    }

    fn new(
        temp: TempDir,
        root: PathBuf,
        document_name: &str,
        parent: Option<PathBuf>,
    ) -> Result<Self, String> {
        let document = root.join(document_name);
        Ok(Self {
            parent,
            _temp: temp,
            document_uri: path_to_uri(&document)?,
            root_uri: path_to_uri(&root)?,
            document,
            root,
        })
    }

    pub(crate) fn create(
        source: &str,
        script_path: Option<&Path>,
        key: &str,
        data: Option<&GeneratedContext>,
    ) -> Result<Self, String> {
        let mut directives = Directives::parse(source)?;
        if let Some(data) = data {
            directives.add_data_packages(&data.connection.kind, &data.csharp);
        }
        let temp = project_directory()?;
        let temporary = temp.path().join("shadow");
        let neighbour = script_path
            .and_then(Path::parent)
            .filter(|parent| parent.is_dir())
            .map(|parent| parent.join(".codingpad").join(short_hash(key)));
        let (root, persistent) = match neighbour {
            Some(root) if prepare(&root).is_ok() => {
                if let Some(parent) = root.parent() {
                    sweep_stale(parent, &root);
                }
                (root, true)
            }
            _ => {
                prepare(&temporary).map_err(|error| {
                    format!("Could not create a code-intelligence workspace: {error}")
                })?;
                (temporary, false)
            }
        };
        let parent = persistent.then(|| root.parent().unwrap().to_path_buf());

        let project = Self::new(temp, root, "Program.cs", parent)?;
        let root = project.root();
        project.write_source(&strip_directives(source))?;
        for (name, contents) in RUNTIME_SOURCES {
            fs::write(root.join("CodingPad").join(name), contents).map_err(|error| {
                format!("Could not stage the CodingPad library for code intelligence: {error}")
            })?;
        }
        fs::write(root.join("CodingPadUsings.cs"), directives.usings()).map_err(|error| {
            format!("Could not stage the CodingPad imports for code intelligence: {error}")
        })?;
        if let Some(data) = data {
            fs::write(root.join("CodingPadDataContext.cs"), &data.csharp)
                .map_err(|error| format!("Could not stage the generated data context: {error}"))?;
        }

        let sdk = if directives.web {
            "Microsoft.NET.Sdk.Web"
        } else {
            "Microsoft.NET.Sdk"
        };
        let data_source = if data.is_some() {
            "    <Compile Include=\"CodingPadDataContext.cs\" />\n"
        } else {
            ""
        };
        let manifest = format!(
            "<Project Sdk=\"{sdk}\">\n  <PropertyGroup>\n{}\n  </PropertyGroup>\n  <ItemGroup>\n{}\n    <Compile Include=\"Program.cs\" />\n    <Compile Include=\"CodingPadUsings.cs\" />\n{data_source}    <Compile Include=\"CodingPad/*.cs\" />\n  </ItemGroup>\n</Project>\n",
            directives.property_manifest(),
            directives.package_manifest(),
        );
        fs::write(root.join("CodingPad.Script.csproj"), manifest).map_err(|error| {
            format!("Could not generate the code-intelligence project: {error}")
        })?;

        project.touch();
        Ok(project)
    }

    pub(crate) fn write_source(&self, source: &str) -> Result<(), String> {
        fs::write(&self.document, source)
            .map_err(|error| format!("Could not stage the script for code intelligence: {error}"))
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn document_uri(&self) -> &str {
        &self.document_uri
    }

    pub(crate) fn root_uri(&self) -> &str {
        &self.root_uri
    }

    /// Refresh the liveness timestamp used by stale-project sweeps.
    pub(crate) fn touch(&self) {
        let _ = fs::write(self.root.join(".codingpad-alive"), b"1");
    }
}

fn project_directory() -> Result<TempDir, String> {
    tempfile::tempdir()
        .map_err(|error| format!("Could not create a code-intelligence workspace: {error}"))
}

impl Drop for ShadowProject {
    fn drop(&mut self) {
        if let Some(parent) = &self.parent {
            let _ = fs::remove_dir_all(&self.root);
            // Fails harmlessly while another script in the folder is open.
            let _ = fs::remove_dir(parent);
        }
    }
}

fn sweep_stale(directory: &Path, current: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(7 * 24 * 60 * 60);
    for entry in entries.flatten() {
        let path = entry.path();
        if path == current || !path.is_dir() {
            continue;
        }
        let marker = path.join(".codingpad-alive");
        let stale = fs::metadata(&marker)
            .or_else(|_| entry.metadata())
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| modified < cutoff);
        if stale {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn canonical_property(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "allowunsafeblocks" => "AllowUnsafeBlocks",
        "checkforoverflowunderflow" => "CheckForOverflowUnderflow",
        "optimize" => "Optimize",
        "treatwarningsaserrors" => "TreatWarningsAsErrors",
        "publishaot" => "PublishAot",
        "packastool" => "PackAsTool",
        "codingpadimplicitusings" => "CodingPadImplicitUsings",
        "nullable" => "Nullable",
        "implicitusings" => "ImplicitUsings",
        "langversion" => "LangVersion",
        "defineconstants" => "DefineConstants",
        "nowarn" => "NoWarn",
        "warningsaserrors" => "WarningsAsErrors",
        "warningsnotaserrors" => "WarningsNotAsErrors",
        "warninglevel" => "WarningLevel",
        "targetframework" => "TargetFramework",
        _ => return None,
    })
}

/// Mirrors `ScriptDirectives.AddDataPackages` in the C# kernel.
fn data_packages(kind: &str, source: &str) -> Vec<(&'static str, &'static str)> {
    let provider = match kind {
        "postgres" => Some("Npgsql.EntityFrameworkCore.PostgreSQL"),
        "sqlserver" => Some("Microsoft.EntityFrameworkCore.SqlServer"),
        _ => Some("Microsoft.EntityFrameworkCore.Sqlite"),
    };
    let mut packages = vec![("Microsoft.EntityFrameworkCore", "10.0.0")];
    if let Some(provider) = provider {
        packages.push((provider, "10.0.0"));
    }
    if kind == "postgres" && source.contains(crate::data_context::CSHARP_NODATIME_MARKER) {
        packages.push(("Npgsql.EntityFrameworkCore.PostgreSQL.NodaTime", "10.0.0"));
    }
    packages
}

fn prepare(root: &Path) -> std::io::Result<()> {
    if root.exists() {
        fs::remove_dir_all(root)?;
    }
    fs::create_dir_all(root.join("CodingPad"))
}

fn short_hash(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(value.as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn strip_directives(source: &str) -> String {
    let mut in_block_comment = false;
    let mut stripping = true;
    let mut lines = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if stripping {
            if in_block_comment {
                if trimmed.contains("*/") {
                    in_block_comment = false;
                }
            } else if trimmed.starts_with("/*") {
                in_block_comment = !trimmed.contains("*/");
            } else if trimmed.is_empty() || trimmed.starts_with("//") {
            } else if trimmed.starts_with("#:") {
                lines.push(format!("// {trimmed}"));
                continue;
            } else {
                stripping = false;
            }
        }
        lines.push(line.to_owned());
    }
    let mut result = lines.join("\n");
    if source.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn unquote(value: &str) -> Result<String, String> {
    if !value.starts_with('"') {
        return Ok(value.to_owned());
    }
    if !value.ends_with('"') || value.len() < 2 {
        return Err("Expected a quoted C# string value.".to_owned());
    }
    let mut result = String::new();
    let mut chars = value[1..value.len() - 1].chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            result.push(character);
            continue;
        }
        match chars.next() {
            Some('n') => result.push('\n'),
            Some('r') => result.push('\r'),
            Some('t') => result.push('\t'),
            Some('0') => result.push('\0'),
            Some('\\') => result.push('\\'),
            Some('"') => result.push('"'),
            Some(other) => {
                result.push('\\');
                result.push(other);
            }
            None => return Err("Expected a quoted C# string value.".to_owned()),
        }
    }
    Ok(result)
}

fn validate_property(name: &str, value: &str) -> Result<(), String> {
    let boolean = || {
        if value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false") {
            Ok(())
        } else {
            Err(format!("{name} must be true or false."))
        }
    };
    match name.to_ascii_lowercase().as_str() {
        "allowunsafeblocks"
        | "checkforoverflowunderflow"
        | "optimize"
        | "treatwarningsaserrors"
        | "publishaot"
        | "packastool"
        | "codingpadimplicitusings" => boolean(),
        "nullable" => match value {
            "enable" | "disable" | "warnings" | "annotations" => Ok(()),
            _ => Err("Invalid Nullable value.".to_owned()),
        },
        "implicitusings" => match value {
            "enable" | "disable" | "true" | "false" => Ok(()),
            _ => Err("Invalid ImplicitUsings value.".to_owned()),
        },
        "langversion" => Ok(()),
        "defineconstants" => {
            if value
                .split([';', ','])
                .map(str::trim)
                .filter(|identifier| !identifier.is_empty())
                .any(|identifier| {
                    !identifier.chars().enumerate().all(|(index, c)| {
                        (c == '_' || c.is_alphabetic()) || (index > 0 && c.is_numeric())
                    })
                })
            {
                Err(
                    "DefineConstants must contain C# identifiers separated by semicolons."
                        .to_owned(),
                )
            } else {
                Ok(())
            }
        }
        "nowarn" | "warningsaserrors" | "warningsnotaserrors" => Ok(()),
        "warninglevel" => match value.parse::<u32>() {
            Ok(level) if level <= 9999 => Ok(()),
            _ => Err("Invalid WarningLevel.".to_owned()),
        },
        "targetframework" => {
            if value == "net10.0" {
                Ok(())
            } else {
                Err("TargetFramework must match the selected runtime: net10.0.".to_owned())
            }
        }
        _ => Err(format!("Unsupported compiler property: {name}")),
    }
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_directives_and_ignores_code_and_comments() {
        let source = "#:package Newtonsoft.Json@13.0.3\n#:package Bogus@35.0.1\n// note\n#:sdk Microsoft.NET.Sdk.Web\n#:property LangVersion=\"preview\"\n#:property Nullable=disable\nvar x = 1;";
        let directives = Directives::parse(source).unwrap();
        assert_eq!(directives.packages["Newtonsoft.Json"], "13.0.3");
        assert!(directives.web);
        assert_eq!(directives.properties["LangVersion"], "preview");
        assert_eq!(directives.properties["Nullable"], "disable");
    }

    #[test]
    fn reports_invalid_directives_with_lines() {
        assert!(Directives::parse("#:package NoVersion")
            .unwrap_err()
            .contains("line 1"));
        assert!(Directives::parse("#:project other.csproj")
            .unwrap_err()
            .contains("not supported"));
        assert!(Directives::parse("#:property Nullable=sometimes")
            .unwrap_err()
            .contains("Invalid Nullable"));
        assert!(Directives::parse("#:unknown value")
            .unwrap_err()
            .contains("Unsupported file directive"));
    }

    #[test]
    fn generates_a_project_with_packages_properties_and_the_runtime() {
        let source = "#:package Newtonsoft.Json@13.0.3\n#:property Nullable=disable\n#:property CodingPadImplicitUsings=false\nConsole.WriteLine(\"hi\");";
        let directives = Directives::parse(source).unwrap();
        let project = ShadowProject::create(source, None, &directives.key(), None).unwrap();
        let csproj = fs::read_to_string(project.root().join("CodingPad.Script.csproj")).unwrap();
        assert!(csproj.contains("Microsoft.NET.Sdk\""));
        assert!(csproj.contains("Newtonsoft.Json"));
        assert!(csproj.contains("<Nullable>disable</Nullable>"));
        assert!(csproj.contains("CodingPad/*.cs"));
        let program = fs::read_to_string(project.root().join("Program.cs")).unwrap();
        assert!(program.starts_with("// #:package"));
        assert!(program.contains("Console.WriteLine(\"hi\");"));
        assert!(!program.contains("\n#:"));
        assert_eq!(
            fs::read_to_string(project.root().join("CodingPadUsings.cs")).unwrap(),
            ""
        );
    }

    #[test]
    fn keeps_code_lines_that_look_like_directives() {
        let program = strip_directives("var text = \"\"\"\n#:package Nope@1\n\"\"\";\n");
        assert!(program.contains("#:package Nope@1"));
    }

    #[test]
    fn restart_key_changes_with_directives() {
        let first = Directives::parse("#:package A@1").unwrap();
        let second = Directives::parse("#:package A@2").unwrap();
        assert_ne!(first.key(), second.key());
        assert_eq!(
            Directives::parse("#:property Optimize=true").unwrap().key(),
            Directives::parse("#:property Optimize=true").unwrap().key()
        );
    }

    #[test]
    fn keeps_the_kernel_usings_when_sdk_implicit_usings_are_disabled() {
        let directives = Directives::parse("#:property ImplicitUsings=disable").unwrap();
        let usings = directives.usings();
        assert!(usings.contains("global using System;"));
        assert!(usings.contains("global using System.Linq;"));
        assert!(!usings.contains("global using System.IO;"));
        assert!(!usings.contains("global using System.Net.Http;"));
        assert!(usings.contains("global using CodingPad;"));
        assert!(usings.contains("global using static CodingPad.ScriptHelpers;"));
    }

    #[test]
    fn treats_property_names_case_insensitively() {
        let directives = Directives::parse(
            "#:property codingpadimplicitusings=false\n#:property nullable=disable",
        )
        .unwrap();
        assert_eq!(directives.usings(), "");
        let manifest = directives.property_manifest();
        assert!(manifest.contains("<Nullable>disable</Nullable>"));
        assert!(!manifest.contains("codingpadimplicitusings"));
        assert!(!manifest.contains("CodingPadImplicitUsings"));
    }

    #[test]
    fn hidden_projects_use_file_based_program_parsing() {
        let directives = Directives::default();
        assert!(directives.property_manifest().contains("FileBasedProgram"));
    }

    #[test]
    fn generated_data_context_is_compiled_with_its_provider_packages() {
        let data = crate::data_context::GeneratedContext {
            csharp: "public sealed class DataContext { }\n".into(),
            connection: crate::data_context::ConnectionInfo {
                kind: "postgres".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let project =
            ShadowProject::create("new DataContext();", None, "key", Some(&data)).unwrap();
        let csproj = fs::read_to_string(project.root().join("CodingPad.Script.csproj")).unwrap();
        assert!(csproj.contains("CodingPadDataContext.cs"));
        assert!(csproj.contains("Microsoft.EntityFrameworkCore"));
        assert!(csproj.contains("Npgsql.EntityFrameworkCore.PostgreSQL"));
        assert!(!csproj.contains("NodaTime"));
        assert_eq!(
            fs::read_to_string(project.root().join("CodingPadDataContext.cs")).unwrap(),
            data.csharp
        );
    }

    #[test]
    fn a_data_context_with_intervals_adds_the_nodatime_package() {
        let data = crate::data_context::GeneratedContext {
            csharp:
                "options.UseNpgsql(builder.ConnectionString, npgsql => npgsql.UseNodaTime());\n"
                    .into(),
            connection: crate::data_context::ConnectionInfo {
                kind: "postgres".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let project =
            ShadowProject::create("new DataContext();", None, "key", Some(&data)).unwrap();
        let csproj = fs::read_to_string(project.root().join("CodingPad.Script.csproj")).unwrap();
        assert!(csproj.contains("Npgsql.EntityFrameworkCore.PostgreSQL.NodaTime"));
    }

    #[test]
    fn saved_scripts_get_a_hidden_neighbour_project() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("script.cs");
        let project = ShadowProject::create("42", Some(&script), "key", None).unwrap();
        assert_eq!(
            project.root(),
            directory.path().join(".codingpad").join(short_hash("key"))
        );
        let root = project.root().to_path_buf();
        let hidden = directory.path().join(".codingpad");
        drop(project);
        assert!(!root.exists());
        assert!(!hidden.exists());
    }
}
