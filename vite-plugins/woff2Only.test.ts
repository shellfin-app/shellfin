import { describe, expect, it } from "vitest";
import { dropWoffFallback } from "./woff2Only";

describe("woff2Only", () => {
  it("drops the woff fallback and keeps the woff2 source and the rest of the rule", () => {
    const css =
      "@font-face {\n  font-family: 'Inter';\n  src: url(./files/inter-latin-400-normal.woff2) format('woff2'), url(./files/inter-latin-400-normal.woff) format('woff');\n  unicode-range: U+0000-00FF;\n}";
    const out = dropWoffFallback(css);
    expect(out).toContain("url(./files/inter-latin-400-normal.woff2) format('woff2');");
    expect(out).not.toContain(".woff)");
    expect(out).toContain("unicode-range: U+0000-00FF;");
  });

  it("leaves css without woff sources alone", () => {
    const css = "a { color: red; }";
    expect(dropWoffFallback(css)).toBe(css);
  });
});
