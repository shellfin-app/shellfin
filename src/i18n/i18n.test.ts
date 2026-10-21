import { describe, expect, it } from "vitest";
import en from "./locales/en/translation.json";
import { KEY_FAMILIES, PROMPT_ERRORS } from "./keys";
import { pseudoLocalize } from "./pseudo";
import { resolveLocale } from "./index";

function lookup(path: string): unknown {
  let node: unknown = en;
  for (const part of path.split(".")) {
    if (typeof node !== "object" || node === null) return undefined;
    node = (node as Record<string, unknown>)[part];
  }
  return node;
}

describe("i18n completeness (§8.2.4)", () => {
  for (const [family, values] of Object.entries(KEY_FAMILIES)) {
    it(`${family}.* has a string for every backend enum value`, () => {
      const missing = values.filter((v) => typeof lookup(`${family}.${v}`) !== "string");
      expect(missing).toEqual([]);
    });
  }

  it("prompt.errors.* has plural forms for every PromptLastError", () => {
    const missing = PROMPT_ERRORS.flatMap((e) =>
      ["one", "other"].map((form) => `prompt.errors.${e}_${form}`).filter((k) => typeof lookup(k) !== "string"),
    );
    expect(missing).toEqual([]);
  });

  it("contains no empty strings", () => {
    const empty: string[] = [];
    const walk = (node: unknown, path: string) => {
      if (typeof node === "string") {
        if (node.trim() === "") empty.push(path);
      } else if (node && typeof node === "object") {
        for (const [k, v] of Object.entries(node)) walk(v, path ? `${path}.${k}` : k);
      }
    };
    walk(en, "");
    expect(empty).toEqual([]);
  });

  it("seeds the §8.2.5 table keys", () => {
    for (const key of [
      "shell.search.placeholder", "shell.newConnection", "hosts.title", "hosts.myBookmarks", "hosts.add",
      "hosts.sshConfigHint", "welcome.title", "welcome.quickConnect.placeholder", "status.hostKeyUnverified",
      "reconnect.title", "terminal.outputDropped", "prompt.passphrase.skip", "hostKey.trust",
    ]) {
      expect(typeof lookup(key), key).toBe("string");
    }
  });
});

describe("en-XA pseudo-locale", () => {
  it("accents letters, pads and keeps interpolation tokens", () => {
    const out = pseudoLocalize("Hosts from {{file}} are loaded");
    expect(out.startsWith("[")).toBe(true);
    expect(out.endsWith("]")).toBe(true);
    expect(out).toContain("{{file}}");
    expect(out).not.toContain("Hosts");
    expect(out.length).toBeGreaterThan("Hosts from {{file}} are loaded".length);
  });
});

describe("resolveLocale", () => {
  it("falls back to en", () => {
    expect(resolveLocale("system", ["de", "fr-CA"])).toBe("en");
    expect(resolveLocale("system", ["en-GB"])).toBe("en");
    expect(resolveLocale("fr")).toBe("en");
  });

  it("detects Ukrainian from the system languages or an explicit setting", () => {
    expect(resolveLocale("system", ["uk-UA", "de"])).toBe("uk");
    expect(resolveLocale("system", ["de", "uk"])).toBe("uk");
    expect(resolveLocale("uk")).toBe("uk");
    expect(resolveLocale("en")).toBe("en");
  });
});
