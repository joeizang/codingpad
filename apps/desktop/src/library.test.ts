import { expect, test } from "vitest";
import { baseName, headerSummary, joinPath, parentPath } from "./library";

test("paths work with either separator", () => {
  expect(baseName("/library/hello.py")).toBe("hello.py");
  expect(baseName("C:\\library\\hello.py")).toBe("hello.py");
  expect(parentPath("/library/sub/hello.py")).toBe("/library/sub");
  expect(joinPath("/library", "hello.py")).toBe("/library/hello.py");
  expect(joinPath("/library/", "hello.py")).toBe("/library/hello.py");
  expect(joinPath("C:\\library", "hello.py")).toBe("C:\\library\\hello.py");
});

test("header settings render as a summary", () => {
  expect(
    headerSummary({
      line: "# codingpad: connection=northwind readOnly",
      settings: { connection: "northwind", readOnly: "" },
    }),
  ).toBe("connection=northwind · readOnly");
  expect(headerSummary(null)).toBe("");
});
