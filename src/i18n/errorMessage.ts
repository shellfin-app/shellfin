// Backend errors are never prose (docs/ARCHITECTURE.md §8.2.2): `errors.<code>` + params, with enum params
// translated through their own key families.
import i18next from "i18next";
import type { AppError } from "../ipc/types";
import { displayServerText } from "../lib/serverText";
import { formatBytes, formatNumber } from "./format";

export function errorMessage(error: AppError): string {
  const t = i18next.t.bind(i18next) as (key: string, options?: Record<string, unknown>) => string;
  // Params can carry server-originated text (remote paths, host names ...): neutralise controls/bidi and bound
  // the length (§9.5). Enum params are replaced by their translations below.
  const params: Record<string, unknown> = {};
  if ("params" in error) {
    for (const [k, v] of Object.entries(error.params as Record<string, unknown>)) {
      params[k] =
        typeof v === "string"
          ? displayServerText(v, 256)
          : Array.isArray(v)
            ? v.map((x) => (typeof x === "string" ? displayServerText(x, 256) : x))
            : v;
    }
  }
  switch (error.code) {
    case "notFound":
      params["what"] = t(`entity.${error.params.what}`);
      break;
    case "io":
      params["op"] = t(`ioOp.${error.params.op}`);
      break;
    case "timeout":
      params["op"] = t(`timeoutOp.${error.params.op}`);
      break;
    case "treeTooLarge":
      // depth: levels, entries / directory: entries, memory: bytes.
      params["limit"] = t(`treeLimit.${error.params.limit}`, {
        value: error.params.limit === "memory" ? formatBytes(error.params.value) : formatNumber(error.params.value),
      });
      break;
    case "keyLoadFailed":
      params["reason"] = t(`keyLoadReason.${error.params.reason}`);
      break;
    case "importInvalidFile":
      params["reason"] = t(`importInvalidReason.${error.params.reason}`);
      break;
    default:
      break;
  }
  return t(`errors.${error.code}`, params);
}
