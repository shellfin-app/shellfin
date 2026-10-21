import { loadEnv } from "vite";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
// @ts-expect-error type error without @types/node package
import process from "node:process";
import { freezeSafeXterm } from "./vite-plugins/freezeSafe";
import { woff2Only } from "./vite-plugins/woff2Only";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(({ mode }) => {
  // The mock backend may only be built into non-production modes (dev server, `--mode smoke`). A production
  // build with VITE_BACKEND=mock (env or .env file) is refused, so release artifacts cannot contain it.
  const env = loadEnv(mode, ".", "VITE_");
  if (mode === "production" && (env["VITE_BACKEND"] === "mock" || process.env.VITE_BACKEND === "mock")) {
    throw new Error("VITE_BACKEND=mock is not allowed in a production build (use `--mode smoke`, see README).");
  }

  return {
    plugins: [freezeSafeXterm(), woff2Only(), react()],

    // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
    //
    // 1. prevent Vite from obscuring rust errors
    clearScreen: false,
    // 2. tauri expects a fixed port, fail if that port is not available
    server: {
      // 1420 (scaffold default) can lie in a Windows/Hyper-V excluded TCP port range (EACCES), so 5173 is used.
      port: 5173,
      strictPort: true,
      host: host || false,
      hmr: host
        ? {
            protocol: "ws",
            host,
            port: 5174,
          }
        : undefined,
      watch: {
        // 3. tell Vite to ignore watching `src-tauri`
        ignored: ["**/src-tauri/**"],
      },
    },

    // The xterm bundles must go through the freeze-safe transform, so they are not pre-bundled in dev.
    optimizeDeps: { exclude: ["@xterm/xterm", "@xterm/addon-webgl"] },

    // Local desktop bundle: the single-chunk size warning is not meaningful.
    build: { target: "es2022", chunkSizeWarningLimit: 2000 },

    test: {
      environment: "jsdom",
      include: ["src/**/*.test.{ts,tsx}", "vite-plugins/**/*.test.ts", "scripts/**/*.test.mjs"],
      setupFiles: ["src/test/setup.ts"],
      css: false,
    },
  };
});
