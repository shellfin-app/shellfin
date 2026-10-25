import { defineConfig } from "i18next-cli";

// Key extraction / drift check (docs/ARCHITECTURE.md §8.2.4).
// `{{namespace}}` in `output` is REQUIRED: without it i18next-cli switches to merged-namespace files.
// Only English is managed here: it is the one complete, CI-enforced locale. Listing other locales would make
// `extract` write "" for every key they have not translated yet (i18next-cli has no option to skip them) and make
// `i18n:check` fail on an untranslated key. Other locales may leave keys out (English fallback); `npm run i18n:extract`
// prunes their stale keys afterwards and `npm run i18n:missing` lists what is untranslated (scripts/i18n-missing.mjs).
export default defineConfig({
  locales: ["en"],
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
