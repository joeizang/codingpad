//! PEP 723 metadata; uv owns requirement validation, resolution and environment caching.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Packages {
    #[serde(skip)]
    pub declared: bool,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_python: Option<String>,
}

impl Packages {
    pub fn parse(source: &str) -> Result<Self, String> {
        // Closing markers may occur within TOML multiline strings. Use the
        // last marker in a comment run, stopping at the next metadata block.
        let lines: Vec<_> = source.lines().collect();
        let is_start = |line: &str| {
            line.strip_prefix("# /// ").is_some_and(|kind| {
                !kind.is_empty() && kind.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        };
        let mut script = None;
        let mut index = 0;
        while index < lines.len() {
            let start = index;
            index += 1;
            if !is_start(lines[start]) {
                continue;
            }
            let mut end = None;
            while index < lines.len() {
                let line = lines[index];
                if is_start(line) || !(line == "#" || line.starts_with("# ")) {
                    break;
                }
                if line == "# ///" {
                    end = Some(index);
                }
                index += 1;
            }
            let Some(end) = end else {
                continue;
            }; // Unclosed blocks are ignored.
            if lines[start] != "# /// script" {
                continue;
            }
            if script.is_some() {
                return Err("PEP 723: multiple script metadata blocks".into());
            }
            let content = lines[start + 1..end]
                .iter()
                .map(|line| line.strip_prefix("# ").unwrap_or(""))
                .collect::<Vec<_>>()
                .join("\n");
            let mut packages: Self = toml::from_str(&content)
                .map_err(|e| format!("Invalid PEP 723 script metadata: {e}"))?;
            packages.declared = true;
            packages.dependencies.sort();
            packages.dependencies.dedup();
            script = Some(packages);
        }
        Ok(script.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiline_toml_and_normalizes_dependency_order() {
        let source = "# /// script\r\n# dependencies = [\r\n# 'rich',\r\n# 'requests<3', 'rich',\r\n# ]\r\n#\r\n# requires-python = '>=3.11'\r\n# [tool.example]\r\n# enabled = true\r\n# ///\r\n42";
        let packages = Packages::parse(source).unwrap();
        assert_eq!(packages.dependencies, ["requests<3", "rich"]);
        assert_eq!(packages.requires_python.as_deref(), Some(">=3.11"));
        assert_eq!(Packages::parse("42").unwrap(), Packages::default());
    }

    #[test]
    fn rejects_bad_types_toml_and_duplicate_blocks() {
        for content in [
            "dependencies = 'requests'",
            "dependencies = [1]",
            "requires-python = 3",
            "dependencies = [",
        ] {
            assert!(Packages::parse(&format!("# /// script\n# {content}\n# ///")).is_err());
        }
        assert!(Packages::parse(
            "# /// script\n# dependencies = []\n# ///\n\n# /// script\n# dependencies = []\n# ///"
        )
        .is_err());
    }

    #[test]
    fn ignores_unknown_types_and_unclosed_blocks() {
        for source in [
            "# /// other\n# dependencies = ['ignored']\n# ///",
            "# /// script\n# dependencies = ['unclosed']",
            "# /// script\n#invalid\n# ///",
        ] {
            assert_eq!(Packages::parse(source).unwrap(), Packages::default());
        }
    }

    #[test]
    fn interior_delimiter_can_be_toml_content() {
        let source = "# /// script\n# [tool.example]\n# text = '''\n# ///\n# text\n# '''\n# ///";
        let packages = Packages::parse(source).unwrap();
        assert!(packages.declared);
        assert!(packages.dependencies.is_empty());
    }
}
