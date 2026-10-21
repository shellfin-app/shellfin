// Minimal typing of the Node APIs tests use (styles/contrast.test.ts reads tokens.css and the component CSS from disk,
// because the test config turns CSS processing off and `?raw` CSS imports come back empty). No @types/node dependency.
declare module "node:fs" {
  export function readFileSync(path: string, encoding: "utf8"): string;
  export function readdirSync(path: string, options: { recursive: true }): string[];
}
declare module "node:path" {
  export function join(...parts: string[]): string;
}
