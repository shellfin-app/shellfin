import { defineConfig } from "i18next-cli";

// Key extraction / drift check (docs/ARCHITECTURE.md §8.2.4).
// `{{namespace}}` in `output` is REQUIRED: without it i18next-cli switches to merged-namespace files.
export default defineConfig({
  locales: ["en", "de", "es", "ja", "pt-BR", "uk"],
  extract: {
    input: ["src/**/*.{ts,tsx}"],
    ignore: ["src/**/*.test.{ts,tsx}", "src/ipc/bindings/**"],
    output: "src/i18n/locales/{{language}}/{{namespace}}.json",
    defaultNS: "translation",
    primaryLanguage: "en",
    // Dynamic key families built from backend enums / error codes would otherwise look unused
    // (removeUnusedKeys defaults to true and `extract --ci --dry-run` would exit 1).
    preservePatterns: [
      "errors.*",
      "configWarnings.*",
      "entity.*",
      "ioOp.*",
      "timeoutOp.*",
      "keyLoadReason.*",
      "importInvalidReason.*",
      "importInvalidCode.*",
      "importEntry.*",
      "exportSkipCode.*",
      "disconnectKind.*",
      "authMethod.*",
      "saveStrategy.*",
      "hostKeyVerification.*",
      "prompt.errors.*",
      "session.state.*", // `session.state.${phase}`
      "bookmarkColor.*", // `bookmarkColor.${name}`: the palette (lib/bookmarkColor.ts)
      "activity.portForwarding", // reserved for the port forwarding view, not built in v1 (§1.1)
      "editor.cm.*", // CodeMirror phrase table (lib/cmPhrases.ts)
      "action.*", // `action.${id}`: the action registry (lib/actionIds.ts)
      "menubar.*", // `menubar.${id}`: menu names
      "units.*", // `units.${unit}`: byte units (i18n/format.ts)
    ],
  },
});
