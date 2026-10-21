// The bundled fonts (Inter, JetBrains Mono) come from @fontsource, whose CSS lists a woff2 and a woff file for every
// subset. WebView2, WebKitGTK and WKWebView all support woff2, so the woff fallbacks are dead weight: dropping them
// halves the font files in the bundle.
import type { Plugin } from "vite";

/** `, url(./files/x.woff) format('woff')` after the woff2 source. */
const WOFF_SOURCE = /,\s*url\([^)]*\.woff\)\s*format\(['"]woff['"]\)/g;

export function dropWoffFallback(css: string): string {
  return css.replace(WOFF_SOURCE, "");
}

export function woff2Only(): Plugin {
  return {
    name: "shellfin:woff2-only",
    enforce: "pre",
    transform(code, id) {
      const path = id.split("?")[0]?.replace(/\\/g, "/") ?? "";
      if (!path.includes("/node_modules/@fontsource/") || !path.endsWith(".css")) return null;
      return { code: dropWoffFallback(code), map: null };
    },
  };
}
