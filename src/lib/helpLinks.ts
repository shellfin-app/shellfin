// External pages behind Help menu items (opened through `app_open_external`, after the native confirmation).
export const HELP_LINKS = {
  documentation: "https://shellfin.app",
  securityPolicy: "https://github.com/shellfin-app/shellfin/security/policy",
  /** Feedback on a beta translation (Settings > General): the translation issues, one per language, label `i18n`. */
  translationFeedback: "https://github.com/shellfin-app/shellfin/issues?q=is%3Aissue%20label%3Ai18n",
} as const;
