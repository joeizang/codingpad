export type LspPosition = { line: number; character: number };
export type LspRange = { start: LspPosition; end: LspPosition };

export type LspDiagnostic = {
  range: LspRange;
  severity?: number;
  code?: string | number;
  source?: string;
  message: string;
  relatedInformation?: unknown[];
};

export type LspEvent =
  | {
      method: "textDocument/publishDiagnostics";
      params: {
        uri: string;
        diagnostics?: LspDiagnostic[];
        version?: number;
      };
    }
  | { method: string; params: unknown };

export type LspCompletionItem = {
  tags?: number[];
  label: string | { label: string; detail?: string; description?: string };
  data?: unknown;
  kind?: number;
  detail?: string;
  documentation?: string | { kind: string; value: string };
  insertText?: string;
  insertTextFormat?: number;
  sortText?: string;
  filterText?: string;
  preselect?: boolean;
  commitCharacters?: string[];
  textEdit?:
    | { newText: string; range: LspRange }
    | { newText: string; insert: LspRange; replace: LspRange };
  additionalTextEdits?: { range: LspRange; newText: string }[];
};

export type LspCompletionList = {
  isIncomplete?: boolean;
  items: LspCompletionItem[];
};

export type LspHover = {
  contents:
    | string
    | { kind: string; value: string }
    | (string | { kind: string; value: string })[];
  range?: LspRange;
};

export type DiagnosticsSummary = {
  errors: number;
  warnings: number;
  message?: string;
};

export function diagnosticsFor(event: LspEvent): LspDiagnostic[] {
  if (event.method !== "textDocument/publishDiagnostics") return [];
  return (event.params as { diagnostics?: LspDiagnostic[] }).diagnostics ?? [];
}

export function summarizeDiagnostics(
  diagnostics: LspDiagnostic[],
): { errors: number; warnings: number } {
  let errors = 0;
  let warnings = 0;
  for (const diagnostic of diagnostics) {
    if (diagnostic.severity === 1) errors++;
    else if (diagnostic.severity === 2 || diagnostic.severity === undefined)
      warnings++;
  }
  return { errors, warnings };
}

// Monaco's marker severity values: Hint=1, Info=2, Warning=4, Error=8.
export function markerSeverity(severity?: number): number {
  switch (severity) {
    case 1:
      return 8;
    case 2:
      return 4;
    case 3:
      return 2;
    case 4:
      return 1;
    default:
      return 4;
  }
}

export function diagnosticMarkers(
  diagnostics: LspDiagnostic[],
  fallbackSource = "basedpyright",
) {
  return diagnostics.map((diagnostic) => ({
    startLineNumber: diagnostic.range.start.line + 1,
    startColumn: diagnostic.range.start.character + 1,
    endLineNumber: diagnostic.range.end.line + 1,
    endColumn: diagnostic.range.end.character + 1,
    message: diagnostic.message,
    severity: markerSeverity(diagnostic.severity),
    source: diagnostic.source ?? fallbackSource,
    code: diagnostic.code === undefined ? undefined : String(diagnostic.code),
  }));
}

export function hoverMarkdown(hover: LspHover | null | undefined): string {
  if (!hover) return "";
  const contents = Array.isArray(hover.contents) ? hover.contents : [hover.contents];
  return contents
    .map((content) => (typeof content === "string" ? content : content.value))
    .filter(Boolean)
    .join("\n\n");
}

export function completionLabel(item: LspCompletionItem): string {
  return typeof item.label === "string" ? item.label : item.label.label;
}

export function completionDocumentation(
  item: LspCompletionItem,
): string | undefined {
  if (!item.documentation) return undefined;
  return typeof item.documentation === "string"
    ? item.documentation
    : item.documentation.value;
}

// LSP CompletionItemKind -> Monaco CompletionItemKind.
const COMPLETION_KINDS: Record<number, number> = {
  1: 18, // Text
  2: 0, // Method
  3: 1, // Function
  4: 2, // Constructor
  5: 3, // Field
  6: 4, // Variable
  7: 5, // Class
  8: 7, // Interface
  9: 8, // Module
  10: 9, // Property
  11: 12, // Unit
  12: 13, // Value
  13: 15, // Enum
  14: 17, // Keyword
  15: 27, // Snippet
  16: 19, // Color
  17: 20, // File
  18: 21, // Reference
  19: 23, // Folder
  20: 16, // EnumMember
  21: 14, // Constant
  22: 6, // Struct
  23: 10, // Event
  24: 11, // Operator
  25: 24, // TypeParameter
};

export function completionKind(kind?: number): number {
  return kind === undefined ? 18 : (COMPLETION_KINDS[kind] ?? 18);
}

// A stable value for the C# file directives. Roslyn restarts only when the
// shadow project the kernel would compile has changed.
export function csharpDirectiveKey(source: string): string {
  const directives: string[] = [];
  let inBlockComment = false;
  for (const line of source.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (inBlockComment) {
      if (trimmed.includes("*/")) inBlockComment = false;
      continue;
    }
    if (trimmed.startsWith("/*")) {
      inBlockComment = !trimmed.includes("*/");
      continue;
    }
    if (!trimmed || trimmed.startsWith("//")) continue;
    if (trimmed.startsWith("#:")) {
      directives.push(trimmed);
      continue;
    }
    break;
  }
  return directives.sort().join("\n");
}

// A stable value for the script's inline dependency set. The editor restarts
// basedpyright only when this changes, not on every keystroke.
export function inlineDependencyKey(source: string): string {
  const lines = source.split(/\r?\n/);
  let inside = false;
  let closed = false;
  const body: string[] = [];
  for (const line of lines) {
    const trimmed = line.trimStart();
    if (!inside) {
      if (trimmed === "# /// script") inside = true;
      continue;
    }
    const content = trimmed.replace(/^# ?/, "");
    if (content.trim() === "///") {
      closed = true;
      break;
    }
    body.push(content);
  }
  if (!inside || !closed) return "";
  const metadata = body.join("\n");
  const block = dependencyArray(metadata);
  if (block === null) return "";
  return (block.match(/"([^"]*)"|'([^']*)'/g) ?? [])
    .map((value) => value.slice(1, -1))
    .join("\n");
}

function dependencyArray(metadata: string): string | null {
  const start = metadata.match(/dependencies\s*=\s*\[/);
  if (start?.index === undefined) return null;
  const content = start.index + start[0].length;
  let depth = 1;
  let quote: string | null = null;
  let escaped = false;
  for (let index = content; index < metadata.length; index++) {
    const character = metadata[index];
    if (quote) {
      if (escaped) {
        escaped = false;
      } else if (character === "\\") {
        escaped = true;
      } else if (character === quote) {
        quote = null;
      }
      continue;
    }
    if (character === "#") {
      const newline = metadata.indexOf("\n", index);
      if (newline === -1) break;
      index = newline;
    } else if (character === '"' || character === "'") {
      quote = character;
    } else if (character === "[") {
      depth++;
    } else if (character === "]") {
      depth--;
      if (depth === 0) return metadata.slice(content, index);
    }
  }
  return null;
}

function randomId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

export function uniqueId(prefix: string): string {
  return `${prefix}-${randomId()}`;
}

export const LSP_SESSION = uniqueId("session");
