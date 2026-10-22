import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  type Block,
  type Inline,
  MAX_BLOCKS,
  MAX_INLINE_NODES,
  MAX_NOTES_INPUT,
  allowedReleaseLink,
  parseReleaseNotes,
} from "./releaseNotes";

const plain = (nodes: Inline[]): string =>
  nodes
    .map((n) => (n.kind === "bold" ? plain(n.children) : n.text))
    .join("");

describe("blocks", () => {
  it("parses headings (1-3 hashes), paragraphs and bullet lists", () => {
    const { blocks, truncated } = parseReleaseNotes("# One\n## Two\n### Three\n#### Four\n\nfirst line\nsecond line\n\n- a\n* b");
    expect(truncated).toBe(false);
    expect(blocks.map((b) => b.kind)).toEqual(["heading", "heading", "heading", "paragraph", "paragraph", "list"]);
    expect(blocks.filter((b): b is Extract<Block, { kind: "heading" }> => b.kind === "heading").map((b) => b.level)).toEqual([1, 2, 3]);
    expect(blocks[3]).toEqual({ kind: "paragraph", inline: [{ kind: "text", text: "#### Four" }] });
    expect(blocks[4]).toEqual({ kind: "paragraph", inline: [{ kind: "text", text: "first line second line" }] });
  });

  it("joins indented continuation lines into the item, one nested level", () => {
    const { blocks } = parseReleaseNotes("- top one\n  continues here\n  - nested\n    more nested\n- top two");
    const list = blocks[0];
    expect(list?.kind).toBe("list");
    if (list?.kind !== "list") return;
    expect(list.items).toHaveLength(2);
    expect(plain(list.items[0]?.inline ?? [])).toBe("top one continues here");
    expect(list.items[0]?.sub.map(plain)).toEqual(["nested more nested"]);
    expect(plain(list.items[1]?.inline ?? [])).toBe("top two");
  });

  it("a blank line ends an item; an unindented line ends the list", () => {
    const { blocks } = parseReleaseNotes("- a\n\n  not part of a\nplain");
    expect(blocks.map((b) => b.kind)).toEqual(["list", "paragraph"]);
  });

  it("CRLF input and tabs are handled", () => {
    const { blocks } = parseReleaseNotes("- a\r\n\tb\r\n");
    expect(blocks).toHaveLength(1);
    const list = blocks[0];
    if (list?.kind !== "list") throw new Error("not a list");
    expect(plain(list.items[0]?.inline ?? [])).toBe("a b");
  });

  it("anything else stays literal text", () => {
    const { blocks } = parseReleaseNotes("<img src=x onerror=alert(1)>\n1. numbered\n> quote\n---");
    expect(blocks).toEqual([
      { kind: "paragraph", inline: [{ kind: "text", text: "<img src=x onerror=alert(1)> 1. numbered > quote ---" }] },
    ]);
  });
});

describe("inline", () => {
  it("code, bold and links", () => {
    const { blocks } = parseReleaseNotes("a `c` **b `x`** [t](https://example.com/) z");
    expect(blocks[0]).toEqual({
      kind: "paragraph",
      inline: [
        { kind: "text", text: "a " },
        { kind: "code", text: "c" },
        { kind: "text", text: " " },
        { kind: "bold", children: [{ kind: "text", text: "b " }, { kind: "code", text: "x" }] },
        { kind: "text", text: " " },
        { kind: "link", text: "t", url: "https://example.com/" },
        { kind: "text", text: " z" },
      ],
    });
  });

  it.each(["**open", "`open", "[open](", "[open](x", "[open]", "****", "``", "[](x)", "text **", "a ` b"])(
    "unmatched or empty delimiters are literal: %s",
    (s) => {
      const { blocks } = parseReleaseNotes(s);
      const p = blocks[0];
      expect(p?.kind).toBe("paragraph");
      if (p?.kind === "paragraph") expect(plain(p.inline)).toBe(s);
    },
  );

  describe("link URLs with parentheses", () => {
    const para = (s: string) => {
      const p = parseReleaseNotes(s).blocks[0];
      if (p?.kind !== "paragraph") throw new Error("not a paragraph");
      return p.inline;
    };

    it("the URL ends at the balancing ')': no stray ')' is left as text", () => {
      expect(para("[x](javascript:alert(1))")).toEqual([{ kind: "link", text: "x", url: "javascript:alert(1)" }]);
      expect(allowedReleaseLink("javascript:alert(1)")).toBeNull();
      expect(para("a [x](https://shellfin.app/(a)(b)) z")).toEqual([
        { kind: "text", text: "a " },
        { kind: "link", text: "x", url: "https://shellfin.app/(a)(b)" },
        { kind: "text", text: " z" },
      ]);
    });

    it("extracts the whole URL of a link whose path has parentheses (allowed)", () => {
      const url = "https://github.com/shellfin-app/shellfin/blob/main/a_(b).md";
      const nodes = para(`[w](${url}) tail`);
      expect(nodes[0]).toEqual({ kind: "link", text: "w", url });
      expect(allowedReleaseLink(url)?.pathname).toBe("/shellfin-app/shellfin/blob/main/a_(b).md");
    });

    it("an unbalanced '(' leaves the link literal", () => {
      for (const s of ["[x](https://shellfin.app/(a", "[x](https://shellfin.app/(a) tail", "[x](((", "[x]((a)"]) {
        const nodes = para(s);
        expect(plain(nodes)).toBe(s);
        expect(nodes.every((n) => n.kind === "text")).toBe(true);
      }
    });

    it("whitespace in the destination makes the candidate literal; later links and words stay visible", () => {
      const s = "[ok](https://github.com/shellfin-app/shellfin/releases (beta) see [b](https://shellfin.app/x) and)";
      const nodes = para(s);
      expect(plain(nodes)).toBe(s.replace("[b](https://shellfin.app/x)", "b"));
      expect(nodes.filter((n) => n.kind === "link")).toEqual([{ kind: "link", text: "b", url: "https://shellfin.app/x" }]);
      expect(nodes[0]).toEqual({ kind: "text", text: "[ok](https://github.com/shellfin-app/shellfin/releases (beta) see " });
      for (const lit of ['[a](https://shellfin.app/x "title")', "[a](https://shellfin.app/x\ttab)"]) {
        const n = para(lit);
        expect(n).toEqual([{ kind: "text", text: lit }]);
      }
      // Whitespace after the closing parenthesis does not matter.
      expect(para("[a](https://shellfin.app/x) b")[0]).toEqual({ kind: "link", text: "a", url: "https://shellfin.app/x" });
    });

    it("nesting is limited to 32 levels; deeper makes the link literal", () => {
      const nested = (n: number) => `[x](${"(".repeat(n)}${")".repeat(n)})`;
      // The link's own parenthesis plus n inner pairs: n = 32 is the last allowed.
      expect(para(nested(32))[0]?.kind).toBe("link");
      expect(para(nested(33))[0]?.kind).toBe("text");
    });
  });

  it("input cut in the middle of markup renders as literal leftovers", () => {
    const src = "- **bold item** with `code` and [a link](https://github.com/shellfin-app/shellfin/releases)";
    for (let n = 0; n <= src.length; n++) {
      const { blocks } = parseReleaseNotes(src.slice(0, n));
      for (const b of blocks) expect(["list", "paragraph", "heading"]).toContain(b.kind);
    }
    const { blocks } = parseReleaseNotes(src.slice(0, src.indexOf("](") + 3));
    const list = blocks[0];
    if (list?.kind !== "list") throw new Error("not a list");
    expect(plain(list.items[0]?.inline ?? [])).toContain("[a link](h");
  });
});

describe("limits", () => {
  const timed = (input: string) => {
    const t0 = performance.now();
    const out = parseReleaseNotes(input);
    return { out, ms: performance.now() - t0 };
  };

  it("5000 '**' and 5000 backticks stay bounded and fast", () => {
    for (const s of ["**".repeat(5000), "`".repeat(5000), "[".repeat(5000), "[a](".repeat(2500), "**a `".repeat(2000)]) {
      const { out, ms } = timed(s);
      expect(ms).toBeLessThan(1000);
      expect(out.blocks.length).toBeLessThanOrEqual(MAX_BLOCKS);
      expect(JSON.stringify(out.blocks).length).toBeLessThan(200_000);
    }
  });

  it("unbalanced and deeply nested parentheses stay linear (no rescan per link candidate)", () => {
    const cases = [
      "(".repeat(MAX_NOTES_INPUT),
      `[a](${"(".repeat(MAX_NOTES_INPUT - 4)}`,
      "[a](".repeat(MAX_NOTES_INPUT / 4),
      `${"[a](".repeat(5000)}${")".repeat(5000)}`,
      "[a]((".repeat(8000),
      `[a](${"(".repeat(5000)}${")".repeat(5000)}`,
    ];
    for (const s of cases) {
      const { out, ms } = timed(s);
      expect(ms).toBeLessThan(500);
      expect(out.blocks.length).toBeLessThanOrEqual(MAX_BLOCKS);
    }
  });

  it("many link candidates with whitespace in the destination stay linear", () => {
    const cases = [
      "[a](x y) ".repeat(4000),
      `${"[a](x ".repeat(6000)}${")".repeat(6000)}`,
      `[a](${"x ".repeat(15_000)})`,
      "[a](x\ty) ".repeat(4000),
      `${"[a](".repeat(5000)} ${")".repeat(5000)}`,
    ];
    for (const s of cases) {
      const { out, ms } = timed(s);
      expect(ms).toBeLessThan(500);
      expect(out.blocks.length).toBeLessThanOrEqual(MAX_BLOCKS);
    }
  });

  it("a 10 000-character single line is one paragraph", () => {
    const { out, ms } = timed("x".repeat(10_000));
    expect(ms).toBeLessThan(1000);
    expect(out.blocks).toHaveLength(1);
  });

  it("2000 deeply indented '-' lines become one flat nested level, bounded by the block limit", () => {
    const lines = ["- root"];
    for (let i = 0; i < 2000; i++) lines.push(`${" ".repeat(2 + i)}- item ${i}`);
    const { out, ms } = timed(lines.join("\n"));
    expect(ms).toBeLessThan(1000);
    const list = out.blocks[0];
    if (list?.kind !== "list") throw new Error("not a list");
    expect(list.items).toHaveLength(1);
    expect(list.items[0]?.sub.length).toBeLessThanOrEqual(MAX_BLOCKS);
    expect(out.truncated).toBe(true);
  });

  it("more than MAX_BLOCKS blocks: the rest is dropped and flagged", () => {
    const { out } = timed(Array.from({ length: MAX_BLOCKS + 50 }, (_, i) => `# h${i}`).join("\n"));
    expect(out.blocks).toHaveLength(MAX_BLOCKS);
    expect(out.truncated).toBe(true);
  });

  it("more than MAX_INLINE_NODES inline nodes: the remainder is kept as one plain text node", () => {
    const { out } = timed("`a` ".repeat(MAX_INLINE_NODES));
    expect(out.truncated).toBe(true);
    const nodes = out.blocks.flatMap((b) => (b.kind === "paragraph" ? b.inline : []));
    // The closing plain-text node (and one per bold level) may exceed the budget slightly.
    expect(nodes.length).toBeLessThanOrEqual(MAX_INLINE_NODES + 3);
  });
});

describe("allowedReleaseLink", () => {
  const isAllowedReleaseLink = (u: string) => allowedReleaseLink(u) !== null;

  it.each([
    "https://github.com/shellfin-app/shellfin",
    "https://github.com/shellfin-app/shellfin/",
    "https://github.com/shellfin-app/shellfin?tab=readme#x",
    "https://github.com/shellfin-app/shellfin/releases",
    "https://github.com/shellfin-app/shellfin/releases/",
    "https://github.com/shellfin-app/shellfin/releases/tag/v0.1.1",
    "https://github.com/shellfin-app/shellfin/releases/download/v0.1.1/Shellfin_0.1.1_x64.msi",
    "https://github.com/shellfin-app/shellfin/blob/main/CHANGELOG.md",
    "https://github.com/shellfin-app/shellfin/blob/main/docs/a.md#L3",
    "https://github.com/shellfin-app/shellfin/tree/main",
    "https://github.com/shellfin-app/shellfin/tree/main/",
    "https://github.com/shellfin-app/shellfin/tree/main/src",
    "https://github.com/shellfin-app/shellfin/issues/12",
    "https://github.com/shellfin-app/shellfin/issues/12#issuecomment-1",
    "https://github.com/shellfin-app/shellfin/pull/7",
    "https://github.com/shellfin-app/shellfin/pull/7?x=1",
    "https://shellfin.app/",
    "https://shellfin.app",
    "https://shellfin.app/docs#x",
    "https://shellfin.app/a?b=c",
    "https://www.shellfin.app/",
  ])("allows %s", (u) => expect(isAllowedReleaseLink(u)).toBe(true));

  it.each([
    "javascript:alert(1)",
    "http://github.com/shellfin-app/shellfin",
    "https://github.com.evil.example/shellfin-app/shellfin",
    "https://user@github.com/shellfin-app/shellfin",
    "https://user:pw@github.com/shellfin-app/shellfin",
    "https://github.com:444/shellfin-app/shellfin",
    "https://github.com/shellfin-app/shellfin-evil",
    "https://github.com/shellfin-app/other",
    // Content anyone can author (forks resolve under the same repository path) or other branches.
    "https://github.com/shellfin-app/shellfin/raw/main/x",
    "https://github.com/shellfin-app/shellfin/raw/0123abc/x",
    "https://github.com/shellfin-app/shellfin/commit/0123abc",
    "https://github.com/shellfin-app/shellfin/blob/0123abc/x",
    "https://github.com/shellfin-app/shellfin/blob/dev/x",
    "https://github.com/shellfin-app/shellfin/blob/main",
    "https://github.com/shellfin-app/shellfin/blob/main/",
    "https://github.com/shellfin-app/shellfin/tree/dev",
    "https://github.com/shellfin-app/shellfin/tree/maintenance",
    "https://github.com/shellfin-app/shellfin/issues",
    "https://github.com/shellfin-app/shellfin/issues/",
    "https://github.com/shellfin-app/shellfin/issues/12abc",
    "https://github.com/shellfin-app/shellfin/issues/12/x",
    "https://github.com/shellfin-app/shellfin/pull/7/files",
    "https://github.com/shellfin-app/shellfin/pull/",
    "https://github.com/shellfin-app/shellfin/releases-evil",
    "https://github.com/shellfin-app/shellfin/releases/../raw/0123abc/x",
    "https://github.com/shellfin-app/shellfin/releases/%2e%2e/commit/0123abc",
    "https://github.com/shellfin-app/shellfin-evil/releases",
    // Parsers disagree on these: never clickable.
    "https://github.com/shellfin-app/shellfin/releases/..%2Fcommit",
    "https://github.com/shellfin-app/shellfin/releases/..%2fcommit",
    "https://github.com/shellfin-app/shellfin/releases/..%5Ccommit",
    "https://github.com/shellfin-app/shellfin/releases/..%5ccommit",
    "https://github.com\\shellfin-app\\shellfin",
    "https://github.com\\shellfin-app\\shellfin\\@evil.example",
    "https:\\\\github.com\\shellfin-app\\shellfin",
    "https://github.com/shellfin-app/shellfin/releases/⟨U+202E⟩",
    "https://github.com/",
    "https://gist.github.com/shellfin-app/shellfin",
    "http://shellfin.app/",
    "https://evil.shellfin.app/",
    "https://www2.shellfin.app/",
    "https://www.www.shellfin.app/",
    "https://shellfin.app.evil.example/",
    "https://www.shellfin.app.evil.example/",
    "https://user@shellfin.app/",
    "https://shellfin.app:8443/",
    "https://shellfin.apps/",
    "https://xn--shellfin-app.example/",
    "https://shellfin.app/a b",
    "",
    "not a url",
    `https://shellfin.app/${"a".repeat(2048)}`,
  ])("rejects %j", (u) => expect(isAllowedReleaseLink(u)).toBe(false));

  it("returns the parsed URL, canonicalised", () => {
    expect(allowedReleaseLink("https://GITHUB.COM:443/shellfin-app/shellfin/releases")?.href).toBe(
      "https://github.com/shellfin-app/shellfin/releases",
    );
    expect(allowedReleaseLink("https://@shellfin.app/x")?.href).toBe("https://shellfin.app/x");
  });
});

describe("nested lists are relative to the first bullet", () => {
  const listOf = (src: string) => {
    const list = parseReleaseNotes(src).blocks[0];
    if (list?.kind !== "list") throw new Error("not a list");
    return list;
  };

  it("a uniformly indented list is flat", () => {
    for (const indent of ["  ", "    ", "\t"]) {
      const list = listOf(`${indent}- a\n${indent}- b\n${indent}- c`);
      expect(list.items.map((i) => plain(i.inline))).toEqual(["a", "b", "c"]);
      expect(list.items.every((i) => i.sub.length === 0)).toBe(true);
    }
  });

  it("nests from two columns more than the first bullet", () => {
    const list = listOf("  - a\n    - a1\n    - a2\n  - b\n   - not nested (one more column)");
    expect(list.items.map((i) => plain(i.inline))).toEqual(["a", "b", "not nested (one more column)"]);
    expect(list.items[0]?.sub.map(plain)).toEqual(["a1", "a2"]);
  });
});

describe("headings", () => {
  it("a lone '#' is literal text, not an empty heading", () => {
    const { blocks } = parseReleaseNotes("#\n\n## \n\n# real");
    expect(blocks.map((b) => b.kind)).toEqual(["paragraph", "paragraph", "heading"]);
    expect(blocks[0]).toEqual({ kind: "paragraph", inline: [{ kind: "text", text: "#" }] });
  });
});

describe("the real CHANGELOG 0.1.0 section", () => {
  const changelog = readFileSync("CHANGELOG.md", "utf8").replace(/\r\n/g, "\n");
  const start = changelog.indexOf("## [0.1.0]");
  const body = changelog.slice(changelog.indexOf("\n", start) + 1);

  it("is headings plus lists with joined continuation lines and inline code", () => {
    const { blocks } = parseReleaseNotes(body);
    const kinds = new Set(blocks.map((b) => b.kind));
    expect(kinds.has("heading")).toBe(true);
    expect(kinds.has("list")).toBe(true);
    const headings = blocks.flatMap((b) => (b.kind === "heading" ? [plain(b.inline)] : []));
    expect(headings).toContain("Connections");
    const items = blocks.flatMap((b) => (b.kind === "list" ? b.items : []));
    const wrapped = items.find((i) => plain(i.inline).startsWith("Automatic reconnect with backoff"));
    expect(wrapped).toBeTruthy();
    expect(plain(wrapped?.inline ?? [])).toContain("keeps editor buffers and terminal scrollback; manual disconnect and reconnect;");
    expect(plain(wrapped?.inline ?? [])).not.toContain("\n");
    expect(items.some((i) => i.inline.some((n) => n.kind === "code" && n.text === "ProxyJump"))).toBe(true);
  });
});
