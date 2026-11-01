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
  label: string | { label: string; detail?: string; description?: string };
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

export function diagnosticMarkers(diagnostics: LspDiagnostic[]) {
  return diagnostics.map((diagnostic) => ({
    startLineNumber: diagnostic.range.start.line + 1,
    startColumn: diagnostic.range.start.character + 1,
    endLineNumber: diagnostic.range.end.line + 1,
    endColumn: diagnostic.range.end.character + 1,
    message: diagnostic.message,
    severity: markerSeverity(diagnostic.severity),
    source: diagnostic.source ?? "basedpyright",
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
