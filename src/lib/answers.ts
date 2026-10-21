import type { PromptAnswer } from "../ipc/types";

// Constructors for prompt answers (the discriminant values are protocol data, not UI copy).
export const hostKeyAnswer = (accept: boolean, remember: boolean): PromptAnswer => ({ kind: "hostKey", accept, remember });
export const passphraseAnswer = (passphrase: string, addToAgent: boolean): PromptAnswer => ({
  kind: "passphrase",
  passphrase,
  addToAgent,
});
export const passwordAnswer = (password: string, remember: boolean): PromptAnswer => ({ kind: "password", password, remember });
export const savedPasswordAnswer = (): PromptAnswer => ({ kind: "useSavedPassword" });
export const kbdAnswer = (responses: string[]): PromptAnswer => ({ kind: "keyboardInteractive", responses });
export const skipAnswer = (): PromptAnswer => ({ kind: "skip" });
export const cancelAnswer = (): PromptAnswer => ({ kind: "cancel" });
