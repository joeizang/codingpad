---
status: accepted
---

# Scripts are plain native files, not a CodingPad container format

Each script is saved as an ordinary file in its language (`.py`, `.cs`, `.rs`, `.ts`). It declares its dependencies with that language's official single-file format: PEP 723 for Python, `#:package` for .NET 10 file-based apps, cargo-script frontmatter for Rust, and Bun's versioned imports for TypeScript. CodingPad-only settings (such as the attached connection) live in a single leading comment after any Rust cargo-script frontmatter, e.g. `# codingpad: connection=northwind`. Scripts therefore work in git, in other editors and on the command line without CodingPad. This deliberately departs from LINQPad's proprietary `.linq` format.

## Consequences
- In Rust, cargo-script frontmatter precedes the optional script header so the file remains a Cargo single-file package.
- Anything CodingPad needs to remember per script must fit in that one header line, or be stored app-side (not in the file).
- CodingPad must parse and preserve each language's dependency format rather than owning its own.
