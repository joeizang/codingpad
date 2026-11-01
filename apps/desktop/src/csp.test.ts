import { expect, test } from "vitest";
import config from "../src-tauri/tauri.conf.json";

test("both CSPs allow data: image dumps", () => {
  for (const csp of [config.app.security.csp, config.app.security.devCsp]) {
    expect(csp).toContain("img-src 'self' data:");
  }
});
