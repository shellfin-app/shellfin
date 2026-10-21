// Entry point: fonts, tokens, i18n, bootstrap (docs/ARCHITECTURE.md §8.1). Fonts are bundled locally (CSP).
import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
import "./styles/tokens.css";
import "./styles/global.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { applyDevSwitches, startApp } from "./bootstrap";
import { applyCachedAppearance, installAppearance } from "./features/settings/appearance";
import { initI18n } from "./i18n";
import { useStore } from "./state/store";

async function main() {
  applyCachedAppearance(); // before anything awaits: the last run's theme, so a light theme does not flash dark
  installAppearance(); // then follows settings.appearance / terminal.background and the OS
  await initI18n();
  const container = document.getElementById("root");
  if (!container) throw new Error("#root not found");
  createRoot(container).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
  await startApp();
  if (useStore.getState().ready) await applyDevSwitches(location.search);
}

void main();
