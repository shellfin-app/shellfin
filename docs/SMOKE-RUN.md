# Release smoke run (terminal + CodeMirror under the real CSP)

Goal: in a **release-profile Tauri build**, the terminal and CodeMirror render with no CSP errors, and navigation
to an external URL is blocked. The frontend is built with the in-memory MockBackend (`--mode smoke`) so no SSH
server is needed. Release artifacts can never contain the mock (see `docs/ARCHITECTURE.md` §8.8).

## Build
```
npm run smoke:tauri        # = build:smoke (vite --mode smoke -> dist-smoke) + tauri build --debug --no-bundle --config tauri.smoke.conf.json
```
For a true release profile run the same two steps without `--debug`:
`npm run build:smoke && npx tauri build --no-bundle --config tauri.smoke.conf.json`.
`tauri.smoke.conf.json` only swaps `frontendDist` to `../dist-smoke` and clears `beforeBuildCommand`; CSP,
`freezePrototype`, ACL and navigation lock come from `src-tauri/tauri.conf.json` unchanged.

## Check
1. Start `src-tauri/target/<profile>/shellfin.exe`. The shell renders (Home tab, bookmarks, no blank window).
2. Click a host (e.g. `prod-web-01`): a terminal prompt appears, then click `docker-compose.yml`: CodeMirror shows it.
3. WebView2 devtools / CDP console: no CSP violations, no `TypeError` from xterm (`freezePrototype` stays on).
4. In the console run `location = "https://example.com"`: the navigation is blocked.

## Guards against shipping the mock
- `vite build` in production mode throws if `VITE_BACKEND=mock` is set (env or `.env.*`).
- The mock is behind a build-time constant and a dynamic import, so release bundles do not emit it;
  `npm run build` ends with `scripts/assert-no-mock.mjs dist`, which fails on the marker `__shellfin_mock_backend__`.
- CI: run `npm run build` (includes the assertion) in every job that produces release artifacts.

## Freeze regression gate
`npm run test:freeze` builds the smoke bundle and loads it in jsdom with `Object.freeze(Object.prototype)`, drives
connect -> terminal -> open file, and fails on any exception. `vite-plugins/freezeSafe.ts` rewrites xterm's
`x.toString = y` assignments and fails the build if the expected match count changes after an xterm upgrade.
