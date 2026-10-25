import { readFileSync, readdirSync } from "node:fs";
import { describe, expect, it } from "vitest";
import en from "./locales/en/translation.json";
import { KEY_FAMILIES, PROMPT_ERRORS } from "./keys";
import { pseudoLocalize, pseudoPadding } from "./pseudo";
import { LANGUAGE_ENDONYMS, LANGUAGE_STATUS, SUPPORTED_LANGUAGES, matchLanguage, resolveLocale } from "./index";

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
    expect(resolveLocale("system", ["it", "nl-BE"])).toBe("en");
    expect(resolveLocale("system", ["en-GB"])).toBe("en");
    expect(resolveLocale("it")).toBe("en");
  });

  it("detects Ukrainian from the system languages or an explicit setting", () => {
    expect(resolveLocale("system", ["uk-UA", "de"])).toBe("uk");
    expect(resolveLocale("system", ["it", "uk"])).toBe("uk");
    expect(resolveLocale("uk")).toBe("uk");
    expect(resolveLocale("en")).toBe("en");
  });

  it("detects German, including regional variants", () => {
    expect(resolveLocale("system", ["de-DE", "uk"])).toBe("de");
    expect(resolveLocale("system", ["it", "de-AT"])).toBe("de");
    expect(resolveLocale("system", ["de-CH"])).toBe("de");
    expect(resolveLocale("de")).toBe("de");
  });

  it("detects Spanish, including regional variants", () => {
    expect(resolveLocale("system", ["es-ES", "uk"])).toBe("es");
    expect(resolveLocale("system", ["it", "es-MX"])).toBe("es");
    expect(resolveLocale("system", ["es-419"])).toBe("es");
    expect(resolveLocale("es")).toBe("es");
  });

  it("detects Japanese from the system languages or an explicit setting", () => {
    expect(resolveLocale("system", ["ja-JP", "en"])).toBe("ja");
    expect(resolveLocale("system", ["it", "ja"])).toBe("ja");
    expect(resolveLocale("ja")).toBe("ja");
  });

  it("detects Brazilian Portuguese for every Portuguese variant", () => {
    expect(resolveLocale("system", ["pt-BR", "uk"])).toBe("pt-BR");
    expect(resolveLocale("system", ["it", "pt-PT"])).toBe("pt-BR");
    expect(resolveLocale("system", ["pt"])).toBe("pt-BR");
    expect(resolveLocale("system", ["pt_br"])).toBe("pt-BR");
    expect(resolveLocale("pt-BR")).toBe("pt-BR");
  });

  // The planned first wave (#17) as a fixture list: the rules must work before any of these ships.
  const WAVE = ["en", "uk", "de", "es", "fr", "pt-BR", "ja", "ko", "zh-CN"];
  const table: [string, string | undefined][] = [
    ["zh-CN", "zh-CN"],
    ["ZH-cn", "zh-CN"],
    ["zh", "zh-CN"],
    ["zh-SG", "zh-CN"],
    ["zh-Hans", "zh-CN"],
    ["zh-Hans-CN", "zh-CN"],
    ["zh_CN", "zh-CN"],
    ["zh-TW", undefined],
    ["zh-HK", undefined],
    ["zh-MO", undefined],
    ["zh-Hant", undefined],
    ["zh-Hant-TW", undefined],
    ["pt", "pt-BR"],
    ["pt-BR", "pt-BR"],
    ["pt-PT", "pt-BR"],
    ["pt-AO", "pt-BR"],
    ["es-MX", "es"],
    ["es-419", "es"],
    ["de-AT", "de"],
    ["de-CH", "de"],
    ["fr-CA", "fr"],
    ["ja-JP", "ja"],
    ["ko-KR", "ko"],
    ["uk-UA", "uk"],
    ["en-GB", "en"],
    ["", undefined],
    ["  ", undefined],
    ["x-foo", undefined],
    ["*", undefined],
    ["i-klingon", undefined],
    ["it", undefined],
    ["zz-ZZ", undefined],
    ["de-\u0000", undefined],
  ];
  it.each(table)("matchLanguage(%j) -> %j (first-wave fixture)", (tag, expected) => {
    expect(matchLanguage(tag, WAVE)).toBe(expected);
  });

  it("Traditional Chinese falls through to the next preference, then English", () => {
    expect(resolveLocale("system", ["zh-TW", "ja"], WAVE)).toBe("ja");
    expect(resolveLocale("system", ["zh-Hant-HK"], WAVE)).toBe("en");
    expect(resolveLocale("system", ["x-foo", "pt-PT"], WAVE)).toBe("pt-BR");
    expect(resolveLocale("zh-TW", [], WAVE)).toBe("en");
  });

  it("a rule never returns a language that is not supported", () => {
    expect(matchLanguage("zh-CN")).toBeUndefined();
    expect(matchLanguage("pt-PT", ["en", "uk"])).toBeUndefined();
    expect(resolveLocale("system", ["zh-CN", "uk"])).toBe("uk");
  });
});

describe("registered languages", () => {
  const read = (path: string) => readFileSync(path, "utf8");
  /** NSIS language names (tauri-bundler's bundled translations) of every UI language we may ship. */
  const NSIS: Record<string, string> = {
    en: "English",
    uk: "Ukrainian",
    de: "German",
    es: "Spanish",
    fr: "French",
    "pt-BR": "PortugueseBR",
    ja: "Japanese",
    ko: "Korean",
    "zh-CN": "SimpChinese",
  };

  it("every supported language has a status, an endonym and a locale file", () => {
    for (const lng of SUPPORTED_LANGUAGES) {
      expect(LANGUAGE_STATUS[lng], lng).toMatch(/^(complete|beta)$/);
      expect(LANGUAGE_ENDONYMS[lng], lng).toBeTruthy();
    }
    // Every registered language has a file, and a locale file that is not registered would ship as a chunk nobody
    // can select (and be checked by i18n:check as if it were real).
    const files = readdirSync("src/i18n/locales", { recursive: true })
      .map((p) => p.replaceAll("\\", "/"))
      .filter((p) => p.endsWith("/translation.json"));
    expect(files.map((p) => p.split("/")[0]).sort()).toEqual([...SUPPORTED_LANGUAGES].sort());
    expect(read("i18next.config.ts")).toContain(`locales: [${SUPPORTED_LANGUAGES.map((l) => `"${l}"`).join(", ")}]`);
  });

  it("the Windows installer offers every UI language (NSIS), English first", () => {
    const conf = JSON.parse(read("src-tauri/tauri.conf.json")) as { bundle: { windows: { nsis: { languages: string[] } } } };
    const languages = conf.bundle.windows.nsis.languages;
    expect(languages[0]).toBe("English"); // the fallback when the Windows display language is not in the list
    expect([...languages].sort()).toEqual(SUPPORTED_LANGUAGES.map((l) => NSIS[l]).sort());
  });

  /** Apple localization identifiers of every UI language we may ship (Simplified Chinese is a script, not a region). */
  const APPLE: Record<string, string> = {
    en: "en",
    uk: "uk",
    de: "de",
    es: "es",
    fr: "fr",
    "pt-BR": "pt-BR",
    ja: "ja",
    ko: "ko",
    "zh-CN": "zh-Hans",
  };

  it("the macOS bundle declares every UI language (CFBundleLocalizations in src-tauri/Info.plist)", () => {
    const plist = read("src-tauri/Info.plist");
    const block = /<key>CFBundleLocalizations<\/key>\s*<array>([\s\S]*?)<\/array>/.exec(plist)?.[1] ?? "";
    const declared = [...block.matchAll(/<string>([^<]+)<\/string>/g)].map((m) => m[1]);
    expect([...declared].sort()).toEqual(SUPPORTED_LANGUAGES.map((l) => APPLE[l]).sort());
  });
});

describe("en-XA padding switch (?xa=)", () => {
  it("parses 1 as the default and whole percentages up to 200", () => {
    expect(pseudoPadding(null)).toBeNull();
    expect(pseudoPadding("1")).toBe(0.3);
    expect(pseudoPadding("40")).toBe(0.4);
    expect(pseudoPadding("0")).toBe(0);
    expect(pseudoPadding("200")).toBe(2);
    expect(pseudoPadding("201")).toBeNull();
    expect(pseudoPadding("-5")).toBeNull();
    expect(pseudoPadding("4e1")).toBeNull();
    expect(pseudoPadding("")).toBeNull();
  });

  it("pads by the given share", () => {
    const letters = "abcdefghij"; // 10 letters
    const pad = (s: string) => (s.match(/·/g) ?? []).length;
    expect(pad(pseudoLocalize(letters))).toBe(3);
    expect(pad(pseudoLocalize(letters, 0.4))).toBe(4);
    expect(pad(pseudoLocalize(letters, 0))).toBe(0);
  });
});
