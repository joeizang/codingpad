export type FolderEntry = {
  name: string;
  path: string;
  isDir: boolean;
};

export type ScriptHeader = {
  line: string;
  settings: Record<string, string>;
};

export type LibrarySettings = {
  libraryPath: string;
  pinnedFolders: string[];
};

export type ScriptFile = {
  path: string;
  name: string;
  source: string;
};

// The Cargo fence must precede comments for the file to run with Cargo.
export function cargoScriptFrontmatter(source: string) {
  const lines = source.replace(/^\uFEFF/, "").split(/\r?\n/);
  if (/^#!/.test(lines[0] ?? "") && !/^#!\s*\[/.test(lines[0])) lines.shift();
  const first = lines.findIndex((line) => line.trim() !== "");
  const fence = first < 0 ? null : lines[first].match(/^(-{3,255})(?:[ \t]*cargo)?[ \t]*$/)?.[1];
  if (!fence) return null;
  const end = lines.findIndex((line, index) => index > first && line.trimEnd() === fence);
  return {
    key: lines.slice(first, end === -1 ? first + 1 : end + 1).join("\n"),
    body: end === -1 ? null : lines.slice(end + 1).find((line) => line !== ""),
  };
}

// Mirror of the Rust parser so the footer stays accurate while the header line
// is being edited, before anything is saved.
export function parseHeader(source: string): ScriptHeader | null {
  const first = cargoScriptFrontmatter(source)?.body ?? source.split(/\r?\n/, 1)[0] ?? "";
  const line = first.startsWith("\uFEFF") ? first.slice(1) : first;
  const trimmed = line.trimStart();
  const rest = trimmed.startsWith("//")
    ? trimmed.slice(2)
    : trimmed.startsWith("--")
      ? trimmed.slice(2)
      : trimmed.startsWith("#")
        ? trimmed.slice(1)
        : null;
  if (rest === null) return null;
  const text = rest.trimStart();
  if (!text.startsWith("codingpad:")) return null;
  const settings: Record<string, string> = {};
  for (const token of tokenize(text.slice("codingpad:".length))) {
    const index = token.indexOf("=");
    if (index === -1) {
      if (token) settings[token] = "";
    } else {
      const key = token.slice(0, index);
      if (key) settings[key] = token.slice(index + 1);
    }
  }
  return { line, settings };
}

function tokenize(text: string): string[] {
  const tokens: string[] = [];
  let token = "";
  let quote = "";
  for (const character of text) {
    if (quote) {
      if (character === quote) quote = "";
      else token += character;
    } else if (character === '"' || character === "'") {
      quote = character;
    } else if (/\s/.test(character)) {
      if (token) {
        tokens.push(token);
        token = "";
      }
    } else {
      token += character;
    }
  }
  if (token) tokens.push(token);
  return tokens;
}

export function baseName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.at(-1) ?? path;
}

export function joinPath(directory: string, name: string): string {
  const separator = directory.includes("\\") ? "\\" : "/";
  return `${directory.replace(/[\\/]+$/, "")}${separator}${name}`;
}

export function parentPath(path: string): string {
  const index = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return index > 0 ? path.slice(0, index) : path;
}

export function headerSummary(header: ScriptHeader | null): string {
  if (!header) return "";
  return Object.entries(header.settings)
    .map(([key, value]) => (value ? `${key}=${value}` : key))
    .join(" · ");
}

/** Requirements from a Python script's inline metadata block (PEP 723). */
export function scriptPackages(source: string): string[] {
  const block = /^# \/\/\/ script\s*$([\s\S]*?)^# \/\/\/\s*$/m.exec(source);
  if (!block) return [];
  const toml = block[1]
    .split(/\r?\n/)
    .map((line) => line.replace(/^#( |$)/, ""))
    .join("\n");
  // Extras like "pandas[excel]" contain brackets, so the list ends at a line-final "]".
  const list = /^dependencies\s*=\s*\[([\s\S]*?)\]\s*$/m.exec(toml);
  if (!list) return [];
  return [...list[1].matchAll(/"([^"]+)"|'([^']+)'/g)].map((match) =>
    (match[1] ?? match[2]).trim(),
  );
}
