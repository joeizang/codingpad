import { describe, expect, test } from "vitest";
import {
  completionDocumentation,
  completionKind,
  completionLabel,
  diagnosticMarkers,
  diagnosticsFor,
  hoverMarkdown,
  inlineDependencyKey,
  markerSeverity,
  summarizeDiagnostics,
  type LspDiagnostic,
} from "./lsp";

const diagnostic: LspDiagnostic = {
  range: { start: { line: 2, character: 4 }, end: { line: 2, character: 9 } },
  severity: 1,
  source: "basedpyright",
  code: "reportAssignmentType",
  message: "Type \"str\" is not assignable to declared type \"int\"",
};

describe("language-server diagnostics", () => {
  test("turns LSP ranges into Monaco markers", () => {
    expect(diagnosticMarkers([diagnostic])).toEqual([
      {
        startLineNumber: 3,
        startColumn: 5,
        endLineNumber: 3,
        endColumn: 10,
        message: diagnostic.message,
        severity: 8,
        source: "basedpyright",
        code: "reportAssignmentType",
      },
    ]);
  });

  test("counts errors and warnings without treating info as a problem", () => {
    expect(
      summarizeDiagnostics([
        diagnostic,
        { ...diagnostic, severity: 2 },
        { ...diagnostic, severity: 2 },
        { ...diagnostic, severity: 3 },
      ]),
    ).toEqual({ errors: 1, warnings: 2 });
  });

  test("keeps the LSP severity mapping", () => {
    expect([undefined, 1, 2, 3, 4].map(markerSeverity)).toEqual([
      4, 8, 4, 2, 1,
    ]);
  });

  test("reads diagnostics from a publishDiagnostics notification", () => {
    expect(
      diagnosticsFor({
        method: "textDocument/publishDiagnostics",
        params: { uri: "file:///workspace/script.py", diagnostics: [diagnostic] },
      }),
    ).toEqual([diagnostic]);
    expect(
      diagnosticsFor({ method: "window/logMessage", params: { message: "hi" } }),
    ).toEqual([]);
  });
});

describe("language-server presentation", () => {
  test("flattens hover contents", () => {
    expect(
      hoverMarkdown({
        contents: [
          "First",
          { kind: "markdown", value: "**Second**" },
        ],
      }),
    ).toBe("First\n\n**Second**");
    expect(hoverMarkdown(null)).toBe("");
  });

  test("restarts only when the inline dependency set changes", () => {
    expect(inlineDependencyKey("print(1)")).toBe("");
    const source = '# /// script\n# dependencies = ["requests"]\n# ///\nimport requests';
    expect(inlineDependencyKey(source)).toBe("requests");
    expect(
      inlineDependencyKey(source.replace('"requests"', '"requests", "rich"')),
    ).toBe("requests\nrich");
    const extras =
      '# /// script\n# dependencies = ["requests[security]"]\n# ///';
    expect(inlineDependencyKey(extras)).toBe("requests[security]");
    expect(
      inlineDependencyKey(extras.replace("[security]", "[socks]")),
    ).toBe("requests[socks]");
    expect(
      inlineDependencyKey(
        '# /// script\n# dependencies = [\n#   "requests", # a ] comment\n# ]\n# ///',
      ),
    ).toBe("requests");
  });

  test("maps completion labels, documentation and kinds", () => {
    expect(completionLabel({ label: "print" })).toBe("print");
    expect(
      completionLabel({ label: { label: "greet", detail: "def" } }),
    ).toBe("greet");
    expect(
      completionDocumentation({
        label: "print",
        documentation: { kind: "markdown", value: "Print a value" },
      }),
    ).toBe("Print a value");
    expect(completionKind(3)).toBe(1);
    expect(completionKind(undefined)).toBe(18);
    expect(completionKind(999)).toBe(18);
  });
});
