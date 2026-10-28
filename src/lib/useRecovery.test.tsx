import { renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useRecoverySnapshots } from "./useRecovery";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

function snapshotCalls() {
  return invokeMock.mock.calls.filter(([command]) => command === "recovery_snapshot");
}

describe("useRecoverySnapshots", () => {
  beforeEach(() => {
    invokeMock.mockResolvedValue({ written: false, sessionId: null, bytes: 0 });
  });

  afterEach(() => {
    invokeMock.mockReset();
  });

  it("ticks while the document is dirty and stops once it is clean", async () => {
    const { rerender } = renderHook(
      ({ dirty }: { dirty: boolean }) =>
        useRecoverySnapshots("doc-1", dirty, 20),
      { initialProps: { dirty: true } },
    );

    await waitFor(() => expect(snapshotCalls().length).toBeGreaterThanOrEqual(2));

    rerender({ dirty: false });
    // Returning to the saved revision asks the backend once more so a stale
    // snapshot can be removed, then the ticker stops.
    await waitFor(() => expect(snapshotCalls().length).toBeGreaterThanOrEqual(3));
    const cleanCalls = snapshotCalls().length;
    await new Promise((resolve) => setTimeout(resolve, 70));
    expect(snapshotCalls().length).toBe(cleanCalls);
  });

  it("does not schedule ticks for a clean document or without a document", async () => {
    renderHook(() => useRecoverySnapshots("doc-1", false, 20));
    await waitFor(() => expect(snapshotCalls().length).toBe(1));
    await new Promise((resolve) => setTimeout(resolve, 70));
    expect(snapshotCalls()).toHaveLength(1);

    invokeMock.mockClear();
    renderHook(() => useRecoverySnapshots(null, true, 20));
    await new Promise((resolve) => setTimeout(resolve, 70));
    expect(snapshotCalls()).toHaveLength(0);
  });

  it("keeps ticking when a tick fails", async () => {
    invokeMock.mockRejectedValueOnce(new Error("temporary failure"));

    renderHook(() => useRecoverySnapshots("doc-1", true, 20));

    await waitFor(() => expect(snapshotCalls().length).toBeGreaterThanOrEqual(2));
  });
});
