export const SCRIPT_LANGUAGES = [
  { id: "python", extension: "py", name: "Python", chip: "PY", key: "P", detail: "Scripts, packages and dumps", connection: "optional", comment: "#" },
  { id: "csharp", extension: "cs", name: "C#", chip: "C#", key: "C", detail: "Top-level statements on .NET", connection: "optional", comment: "//" },
  { id: "typescript", extension: "ts", name: "TypeScript", chip: "TS", key: "T", detail: "Scripts on Bun", connection: "optional", comment: "//" },
  { id: "rust", extension: "rs", name: "Rust", chip: "RS", key: "R", detail: "Scripts on Cargo", connection: "optional", comment: "//" },
  { id: "sql", extension: "sql", name: "SQL", chip: "SQL", key: "S", detail: "Query a connected database", connection: "required", comment: "--" },
] as const;

export type ScriptLanguage = typeof SCRIPT_LANGUAGES[number]["id"];

export function scriptLanguageDetails(id: string) {
  return SCRIPT_LANGUAGES.find((language) => language.id === id);
}

export function scriptLanguage({ path, name }: { path?: string; name: string }) {
  const extension = /\.([^.]+)$/.exec(path ?? name)?.[1].toLowerCase();
  return SCRIPT_LANGUAGES.find((language) => language.extension === extension)?.id ?? "plaintext";
}

export function languageLabel(id: string) {
  return scriptLanguageDetails(id)?.name ?? "Text";
}
