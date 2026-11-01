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
