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
  header: ScriptHeader | null;
};

// Mirror of the Rust parser so the footer stays accurate while the header line
// is being edited, before anything is saved.
export function parseHeader(source: string): ScriptHeader | null {
  const first = source.split(/\r?\n/, 1)[0] ?? "";
  const line = first.startsWith("\uFEFF") ? first.slice(1) : first;
  const trimmed = line.trimStart();
  const rest = trimmed.startsWith("//")
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
