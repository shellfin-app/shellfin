import { describe, expect, it } from "vitest";
import { rewriteToStringAssignments } from "./freezeSafe";

describe("freeze-safe rewrite", () => {
  it("rewrites toString assignments to defineProperty", () => {
    const { code, count } = rewriteToStringAssignments("(o=>{function s(l){return 1}o.toString=s;o.x=1})(X||={});");
    expect(count).toBe(1);
    expect(code).toContain('Object.defineProperty(o,"toString",{value:s,writable:true,configurable:true,enumerable:true});');
    expect(code).not.toContain("o.toString=s");
  });

  it("does not touch property reads, member chains or other assignments", () => {
    const src = "a.b.toString=c;x.toString();y.toStringTag=z;";
    expect(rewriteToStringAssignments(src)).toEqual({ code: src, count: 0 });
  });
});
