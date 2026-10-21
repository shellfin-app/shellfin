import { EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";
import { saveTransformChanges } from "./saveTransforms";

function apply(text: string, trim: boolean, finalNewline: boolean): string {
  const state = EditorState.create({ doc: text });
  const changes = saveTransformChanges(state.doc, { trimTrailingWhitespace: trim, insertFinalNewline: finalNewline });
  return state.update({ changes }).state.doc.toString();
}

describe("save-time transforms", () => {
  it("does nothing when both are off", () => {
    expect(apply("a  \nb", false, false)).toBe("a  \nb");
  });

  it("trims spaces and tabs at the end of every line, not inside lines", () => {
    expect(apply("a \t\n  b  c  \n\n  \nd", true, false)).toBe("a\n  b  c\n\n\nd");
  });

  it("adds exactly one final newline, and none to an empty document or one that already has it", () => {
    expect(apply("a", false, true)).toBe("a\n");
    expect(apply("a\n", false, true)).toBe("a\n");
    expect(apply("a\n\n", false, true)).toBe("a\n\n");
    expect(apply("", false, true)).toBe("");
  });

  it("a whitespace-only last line becomes the final line break, not an extra blank line", () => {
    expect(apply("a\n   ", true, true)).toBe("a\n");
    expect(apply("a   ", true, true)).toBe("a\n");
  });

  it("is idempotent", () => {
    const once = apply("x  \ny ", true, true);
    expect(apply(once, true, true)).toBe(once);
  });
});
