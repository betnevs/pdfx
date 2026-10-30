import { describe, expect, it, vi } from "vitest";
import {
  flushPendingEdits,
  hasPendingEdits,
  registerPendingEdit,
} from "./pendingEdits";
describe("pending PDF edits", () => {
  it("waits for an inline edit and shares one commit across concurrent save requests", async () => {
    let finish!: (success: boolean) => void;
    const commit = vi.fn(
      () =>
        new Promise<boolean>((resolve) => {
          finish = resolve;
        }),
    );
    const release = registerPendingEdit("draft-test", commit);
    const first = flushPendingEdits("draft-test");
    const second = flushPendingEdits("draft-test");
    expect(commit).toHaveBeenCalledTimes(1);
    expect(hasPendingEdits("draft-test")).toBe(true);
    finish(true);
    expect(await first).toBe(true);
    expect(await second).toBe(true);
    release();
    expect(hasPendingEdits("draft-test")).toBe(false);
  });
  it("blocks saving on failed validation and isolates documents", async () => {
    const release = registerPendingEdit("invalid-draft", async () => false);
    expect(await flushPendingEdits("other-document")).toBe(true);
    expect(await flushPendingEdits("invalid-draft")).toBe(false);
    release();
    expect(await flushPendingEdits("invalid-draft")).toBe(true);
  });
});
