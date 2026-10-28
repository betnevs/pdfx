import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";

import { getAppInfo } from "./backend";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("getAppInfo", () => {
  afterEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("asks the app_info Tauri command and returns its payload", async () => {
    vi.mocked(invoke).mockResolvedValue({ name: "PDFX", version: "0.1.0" });

    await expect(getAppInfo()).resolves.toEqual({
      name: "PDFX",
      version: "0.1.0",
    });
    expect(invoke).toHaveBeenCalledWith("app_info");
  });

  it("propagates a failing command so callers can degrade gracefully", async () => {
    vi.mocked(invoke).mockRejectedValue(new Error("ipc unavailable"));

    await expect(getAppInfo()).rejects.toThrow("ipc unavailable");
  });
});
