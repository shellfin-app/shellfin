import { describe, expect, it } from "vitest";
import { canRemove, writability } from "./permissions";

const me = { uid: 1000, gids: [1000, 27] };
const file = (permissions: number | null, uid: number | null, gid: number | null) =>
  ({ kind: "file", permissions, uid, gid }) as const;
const dir = (permissions: number | null, uid: number | null, gid: number | null) =>
  ({ kind: "dir", permissions, uid, gid }) as const;

describe("writability", () => {
  it("uses the owner, group and other bits", () => {
    expect(writability(file(0o644, 1000, 1000), me)).toBe("yes");
    expect(writability(file(0o444, 1000, 1000), me)).toBe("no");
    expect(writability(file(0o664, 0, 27), me)).toBe("yes");
    expect(writability(file(0o644, 0, 27), me)).toBe("no");
    expect(writability(file(0o646, 0, 0), me)).toBe("yes");
    expect(writability(file(0o644, 0, 0), me)).toBe("no");
  });

  it("needs the search bit on directories", () => {
    expect(writability(dir(0o755, 1000, 1000), me)).toBe("yes");
    expect(writability(dir(0o755, 0, 0), me)).toBe("no");
    expect(writability(dir(0o722, 0, 0), me)).toBe("no");
    expect(writability(dir(0o723, 0, 0), me)).toBe("yes");
  });

  it("root can write everything and unknown inputs give no hint", () => {
    expect(writability(file(0o000, 5, 5), { uid: 0, gids: [0] })).toBe("yes");
    expect(writability(file(null, 1, 1), me)).toBe("unknown");
    expect(writability(file(0o644, null, 1), me)).toBe("unknown");
    expect(writability(file(0o644, 1, 1), null)).toBe("unknown");
    expect(writability({ kind: "symlink", permissions: 0o777, uid: 0, gid: 0 }, me)).toBe("unknown");
    expect(writability(null, me)).toBe("unknown");
  });
});

describe("canRemove", () => {
  it("needs a writable parent and honours the sticky bit", () => {
    expect(canRemove(dir(0o755, 1000, 1000), { uid: 5 }, me)).toBe("yes");
    expect(canRemove(dir(0o755, 0, 0), { uid: 1000 }, me)).toBe("no");
    expect(canRemove(dir(0o1777, 0, 0), { uid: 1000 }, me)).toBe("yes");
    expect(canRemove(dir(0o1777, 0, 0), { uid: 5 }, me)).toBe("no");
    expect(canRemove(dir(0o1777, 1000, 1000), { uid: 5 }, me)).toBe("yes");
    expect(canRemove(dir(0o1777, 0, 0), { uid: null }, me)).toBe("unknown");
    expect(canRemove(dir(null, 0, 0), { uid: 5 }, me)).toBe("unknown");
  });
});
