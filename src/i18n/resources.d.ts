// Typed translation keys (docs/ARCHITECTURE.md §8.2.2): CustomTypeOptions module augmentation.
import type en from "./locales/en/translation.json";

declare module "i18next" {
  interface CustomTypeOptions {
    defaultNS: "translation";
    resources: { translation: typeof en };
  }
}
