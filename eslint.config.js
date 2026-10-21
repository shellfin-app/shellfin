// ESLint 10 flat config. See docs/ARCHITECTURE.md §8.2.2 (i18n literals) and §9.5 (untrusted server content).
import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import i18next from "eslint-plugin-i18next";

// In flat config a later block's options for a rule REPLACE an earlier block's options wholesale, so both
// i18n blocks spread this one shared object (§8.2.2).
const i18nBase = {
  "jsx-attributes": {
    exclude: [
      "className", "id", "type", "key", "href", "role", "data-*", "name", "htmlFor", "autoComplete", "spellCheck",
      "lang", "dir", "inputMode", "orientation", "variant", "size", "icon", "testId",
    ],
  },
  callees: {
    exclude: ["t", "i18n.t", "logger.*", "console.*", "invoke", "call", "listen", "emit", "classNames", "cn", "Error"],
  },
  words: { exclude: ["^[a-z0-9_.:/~-]+$", "^#[0-9a-fA-F]{3,8}$"] }, // no ^[A-Z_]+$ (would exempt "OK")
};

export default tseslint.config(
  { ignores: ["dist/**", "dist-smoke/**", "node_modules/**", "src-tauri/**", "src/ipc/bindings/**", "coverage/**"] },

  js.configs.recommended,
  ...tseslint.configs.recommended,

  // Node build/CI scripts.
  {
    files: ["scripts/**/*.mjs"],
    languageOptions: { globals: { console: "readonly", process: "readonly", setTimeout: "readonly", Response: "readonly" } },
  },

  {
    files: ["src/**/*.{ts,tsx}"],
    plugins: { "react-hooks": reactHooks },
    rules: {
      ...reactHooks.configs.recommended.rules,
      "@typescript-eslint/no-unused-vars": ["error", { argsIgnorePattern: "^_", varsIgnorePattern: "^_" }],

      // §9.5 — never render server data as HTML (core rules only; eslint-plugin-react does not support ESLint 10).
      "no-restricted-syntax": [
        "error",
        {
          selector: "JSXAttribute[name.name='dangerouslySetInnerHTML']",
          message: "dangerouslySetInnerHTML is forbidden (docs/ARCHITECTURE.md §9.5).",
        },
        {
          selector: "AssignmentExpression[left.property.name=/^(innerHTML|outerHTML)$/]",
          message: "Assigning innerHTML/outerHTML is forbidden (§9.5).",
        },
      ],
      "no-restricted-properties": [
        "error",
        { property: "innerHTML", message: "innerHTML is forbidden (§9.5)." },
        { property: "outerHTML", message: "outerHTML is forbidden (§9.5)." },
        { property: "insertAdjacentHTML", message: "insertAdjacentHTML is forbidden (§9.5)." },
        { object: "document", property: "write", message: "document.write is forbidden (§9.5)." },
        { object: "document", property: "writeln", message: "document.writeln is forbidden (§9.5)." },
      ],
      "no-eval": "error",
      "no-implied-eval": "error",
      "no-new-func": "error",
    },
  },

  // i18n block 1 (listed FIRST): every .tsx file, JSX text and attributes only.
  {
    files: ["src/**/*.tsx"],
    plugins: { i18next },
    rules: { "i18next/no-literal-string": ["error", { ...i18nBase, mode: "jsx-only" }] },
  },
  // i18n block 2 (listed AFTER block 1 so it wins for overlapping files): all string literals.
  {
    files: [
      "src/features/**/*.{ts,tsx}",
      "src/components/**/*.{ts,tsx}",
      "src/state/**/*.ts",
      "src/lib/reportError.ts",
    ],
    plugins: { i18next },
    rules: { "i18next/no-literal-string": ["error", { ...i18nBase, mode: "all" }] },
  },
  // Exempt: ipc, i18n, tests and mocks.
  {
    files: ["src/ipc/**", "src/i18n/**", "src/**/*.test.{ts,tsx}", "src/test/**"],
    rules: { "i18next/no-literal-string": "off" },
  },
);
