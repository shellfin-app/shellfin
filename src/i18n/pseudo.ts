// Dev pseudo-locale "en-XA" (docs/ARCHITECTURE.md §8.2.2): accented letters + ~30 % padding, to surface
// hard-coded strings and layout overflow. Interpolation tokens ({{x}}) and plural suffixes are preserved.

export const PSEUDO_LOCALE = "en-XA";

const MAP: Record<string, string> = {
  a: "á", b: "ƀ", c: "ç", d: "ð", e: "é", f: "ƒ", g: "ĝ", h: "ĥ", i: "í", j: "ĵ", k: "ķ", l: "ļ", m: "ɱ",
  n: "ñ", o: "ó", p: "þ", q: "ƣ", r: "ŕ", s: "š", t: "ţ", u: "ú", v: "ṽ", w: "ŵ", x: "ẋ", y: "ý", z: "ž",
  A: "Á", B: "Ɓ", C: "Ç", D: "Ð", E: "É", F: "Ƒ", G: "Ĝ", H: "Ĥ", I: "Í", J: "Ĵ", K: "Ķ", L: "Ļ", M: "Ṁ",
  N: "Ñ", O: "Ó", P: "Þ", Q: "Ǫ", R: "Ŕ", S: "Š", T: "Ţ", U: "Ú", V: "Ṽ", W: "Ŵ", X: "Ẋ", Y: "Ý", Z: "Ž",
};

/** Pseudo-localize one string. `{{...}}` tokens are left untouched. */
export function pseudoLocalize(source: string): string {
  const parts = source.split(/(\{\{[^}]*\}\})/);
  let letters = 0;
  const body = parts
    .map((part) => {
      if (part.startsWith("{{")) return part;
      let out = "";
      for (const ch of part) {
        const mapped = MAP[ch];
        if (mapped) letters += 1;
        out += mapped ?? ch;
      }
      return out;
    })
    .join("");
  const pad = "·".repeat(Math.ceil(letters * 0.3));
  return `[${body}${pad}]`;
}

/** Recursively pseudo-localize a translation resource object. */
export function pseudoLocalizeBundle(bundle: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(bundle)) {
    if (typeof value === "string") out[key] = pseudoLocalize(value);
    else if (value && typeof value === "object") out[key] = pseudoLocalizeBundle(value as Record<string, unknown>);
    else out[key] = value;
  }
  return out;
}
