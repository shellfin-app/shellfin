// Tauri's `freezePrototype: true` runs `Object.freeze(Object.prototype)` before any page script (§9.1, kept).
// Under that freeze, `obj.toString = fn` on a plain object throws ("override mistake"). @xterm/xterm and
// @xterm/addon-webgl vendor VS Code's `KeyCodeUtils` namespace which does exactly that at module top level,
// so the whole bundle fails to evaluate. This transform rewrites those assignments to Object.defineProperty.
//
// The expected number of rewrites per file is pinned: if an xterm upgrade changes it (more, fewer or moved
// sites), the build FAILS instead of silently regressing. Update EXPECTED after re-running
// `npm run test:freeze` against the new version.
import type { Plugin } from "vite";

interface Target {
  /** Path fragment identifying the file (forward slashes). */
  file: string;
  /** Exact number of `x.toString=y;` statements that must be rewritten. */
  expected: number;
}

export const FREEZE_SAFE_TARGETS: readonly Target[] = [
  { file: "/node_modules/@xterm/xterm/lib/xterm.mjs", expected: 1 },
  { file: "/node_modules/@xterm/addon-webgl/lib/addon-webgl.mjs", expected: 1 },
];

// `o.toString=s;` (minified bundle; identifier = value identifier)
const ASSIGNMENT = /(?<![\w$.])([A-Za-z_$][\w$]*)\.toString=([A-Za-z_$][\w$]*);/g;

export function rewriteToStringAssignments(code: string): { code: string; count: number } {
  let count = 0;
  const out = code.replace(ASSIGNMENT, (_m, target: string, value: string) => {
    count += 1;
    return `Object.defineProperty(${target},"toString",{value:${value},writable:true,configurable:true,enumerable:true});`;
  });
  return { code: out, count };
}

export function freezeSafeXterm(): Plugin {
  return {
    name: "shellfin:freeze-safe-xterm",
    enforce: "pre",
    transform(code, id) {
      const path = id.split("?")[0]?.replace(/\\/g, "/") ?? "";
      const target = FREEZE_SAFE_TARGETS.find((t) => path.endsWith(t.file));
      if (!target) return null;
      const result = rewriteToStringAssignments(code);
      if (result.count !== target.expected) {
        this.error(
          `freeze-safe transform: ${target.file} has ${result.count} \`x.toString=y;\` assignment(s), expected ` +
            `${target.expected}. The xterm version changed: re-check Object.prototype-freeze compatibility ` +
            `(npm run test:freeze) and update vite-plugins/freezeSafe.ts.`,
        );
      }
      return { code: result.code, map: null };
    },
  };
}
