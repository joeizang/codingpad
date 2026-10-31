---
status: accepted
---

# Bun, not Deno, runs TypeScript scripts

TypeScript scripts run on Bun. Inline dependencies are declared with Bun's versioned auto-install imports (`import { z } from "zod@3"`). Bun won because its scripts stay ordinary Node-style TypeScript, it starts fastest, it has built-in SQLite/Postgres/MySQL clients (databases are in the MVP), and it behaves like our other three languages (no per-language permission sandbox).

## Considered options
- **Deno**: better out-of-the-box editor intelligence for inline `npm:` imports, an optional type-check before running, and a permission sandbox. Rejected because the `npm:` prefix makes scripts Deno-specific, and a sandbox in one language out of four gives a false sense of safety. If we want script sandboxing, it should be a CodingPad-wide feature.

## Consequences
- tsserver cannot resolve auto-installed imports, so CodingPad must maintain a hidden project behind each TypeScript script (imports installed into a cache folder) for completions to work. This is the same mechanism Rust scripts need for rust-analyzer.
- Bun does not type-check. Type errors appear only as editor diagnostics.
