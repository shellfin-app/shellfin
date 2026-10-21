// `t` for code outside React components (actions, menus). i18next-cli finds keys by the call name `t(...)`, so
// importing this and calling it with a literal key keeps the key extraction and the unused-key check working.
import i18next from "i18next";

export function t(key: string, options?: Record<string, unknown>): string {
  return (i18next.t as (k: string, o?: Record<string, unknown>) => string)(key, options);
}
