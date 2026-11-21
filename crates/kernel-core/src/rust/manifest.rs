use super::source::source_prefix_len;
use std::{ops::Range, path::Path};
use toml::{Table, Value};

pub(crate) struct Manifest {
    pub(crate) cargo: String,
    pub(super) frontmatter: Range<usize>,
}

impl Manifest {
    pub(crate) fn parse(source: &str, path: Option<&Path>) -> Result<Self, String> {
        let (embedded, frontmatter) = frontmatter(source)?;
        let invalid = |message: &str| format!("Rust Cargo frontmatter: {message}");
        let mut cargo = embedded
            .parse::<Table>()
            .map_err(|error| invalid(&error.to_string()))?;
        for field in ["workspace", "lib", "bin", "example", "test", "bench"] {
            if cargo.contains_key(field) {
                return Err(invalid(&format!(
                    "{field} is unavailable in single-file packages"
                )));
            }
        }
        let package = table(&mut cargo, "package")?;
        for field in [
            "workspace",
            "build",
            "links",
            "autolib",
            "autobins",
            "autoexamples",
            "autotests",
            "autobenches",
        ] {
            if package.contains_key(field) {
                return Err(invalid(&format!(
                    "package.{field} is unavailable in single-file packages"
                )));
            }
        }
        for (key, value) in [
            ("name", "codingpad-script"),
            ("version", "0.0.0"),
            ("edition", "2024"),
        ] {
            package.entry(key).or_insert_with(|| Value::from(value));
        }

        let directory = path
            .map(std::path::absolute)
            .transpose()
            .map_err(|error| invalid(&format!("Could not resolve the script path: {error}")))?;
        let directories = directory.as_deref().and_then(Path::parent);
        dependency_paths(&mut cargo, directories)?;
        if let Some(targets) = cargo.get_mut("target").and_then(Value::as_table_mut) {
            for target in targets
                .iter_mut()
                .filter_map(|(_, value)| value.as_table_mut())
            {
                dependency_paths(target, directories)?;
            }
        }
        if let Some(patch) = cargo.get_mut("patch").and_then(Value::as_table_mut) {
            for registry in patch
                .iter_mut()
                .filter_map(|(_, value)| value.as_table_mut())
            {
                make_paths_absolute(registry, directories)?;
            }
        }
        if let Some(replace) = cargo.get_mut("replace").and_then(Value::as_table_mut) {
            make_paths_absolute(replace, directories)?;
        }

        table(&mut cargo, "dependencies")?.insert(
            "codingpad".to_owned(),
            Value::Table(Table::from_iter([(
                "path".to_owned(),
                Value::from("codingpad"),
            )])),
        );
        cargo.insert("workspace".to_owned(), Value::Table(Table::new()));
        Ok(Self {
            cargo: cargo.to_string(),
            frontmatter,
        })
    }
}

fn table<'a>(root: &'a mut Table, key: &str) -> Result<&'a mut Table, String> {
    root.entry(key)
        .or_insert_with(|| Value::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| format!("Rust Cargo frontmatter: {key} must be a table"))
}

fn dependency_paths(root: &mut Table, directory: Option<&Path>) -> Result<(), String> {
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(dependencies) = root.get_mut(key).and_then(Value::as_table_mut) {
            make_paths_absolute(dependencies, directory)?;
        }
    }
    Ok(())
}

fn make_paths_absolute(dependencies: &mut Table, directory: Option<&Path>) -> Result<(), String> {
    for (name, dependency) in dependencies {
        let Some(value) = dependency.get_mut("path") else {
            continue;
        };
        let Some(path) = value.as_str().map(Path::new) else {
            return Err(format!(
                "Rust Cargo frontmatter: {name}.path must be a string"
            ));
        };
        if path.is_relative() {
            let directory = directory.ok_or_else(|| {
                format!("Save the Rust script before using a relative path for dependency {name}.")
            })?;
            *value = Value::from(directory.join(path).to_str().ok_or_else(|| {
                format!("The path for Rust dependency {name} is not valid UTF-8.")
            })?);
        }
    }
    Ok(())
}

fn frontmatter(source: &str) -> Result<(&str, Range<usize>), String> {
    let mut offset = source_prefix_len(source);
    if source[offset..].starts_with('\n') {
        offset += 1; // The shebang terminator.
    }
    let mut lines = source[offset..].split_inclusive('\n');
    let mut opener = "";
    for line in lines.by_ref() {
        if !line.trim().is_empty() {
            opener = line;
            break;
        }
        offset += line.len();
    }
    let dashes = opener.bytes().take_while(|byte| *byte == b'-').count();
    if dashes < 3 {
        return Ok(("", 0..0));
    }
    let fence = &opener[..dashes];
    let info = opener[dashes..].trim();
    let line = source[..offset].lines().count() + 1;
    let invalid = |message: &str| format!("Rust Cargo frontmatter on line {line}: {message}");
    if dashes > 255 || !matches!(info, "" | "cargo") {
        return Err(invalid(
            "Expected a fence of 3 to 255 dashes and an optional cargo identifier.",
        ));
    }
    let body = offset + opener.len();
    let mut end = body;
    for line in lines {
        if line.starts_with(fence) {
            if line.trim_end() != fence {
                return Err(invalid("The closing fence must match the opening fence."));
            }
            return Ok((&source[body..end], offset..end + line.len()));
        }
        end += line.len();
    }
    Err(invalid("Missing closing fence."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_invalid_frontmatter_and_leaves_negative_expressions_alone() {
        for (source, expected) in [
            ("---cargo\n[dependencies]\n", "Missing closing fence"),
            ("---cargo\n[dependencies\n---", "expected"),
            ("---other\n---", "optional cargo identifier"),
            (
                "---cargo\n[workspace]\n---",
                "unavailable in single-file packages",
            ),
        ] {
            let message = Manifest::parse(source, None).err().unwrap();
            assert!(message.contains(expected), "{message}");
        }
        assert!(Manifest::parse("-42", None).unwrap().frontmatter.is_empty());
    }
}
