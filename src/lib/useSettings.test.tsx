import { act, renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { DEFAULT_SETTINGS } from "./settings";
import { useSettings } from "./useSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
});

afterEach(() => {
  document.documentElement.removeAttribute("data-theme");
  document.documentElement.lang = "";
});

describe("useSettings", () => {
  it("loads stored settings and applies the theme and language", async () => {
    invokeMock.mockResolvedValueOnce({
      ...DEFAULT_SETTINGS,
      theme: "dark",
      language: "en-US",
      annotationAuthor: "Alice",
    });

    const { result } = renderHook(() => useSettings());

    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.settings.annotationAuthor).toBe("Alice");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.lang).toBe("en-US");
  });

  it("keeps usable defaults outside the Tauri shell", async () => {
    invokeMock.mockRejectedValueOnce(new Error("not running inside Tauri"));

    const { result } = renderHook(() => useSettings());

    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.settings).toEqual(DEFAULT_SETTINGS);
    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
  });

  it("persists updates and applies the settings the backend returns", async () => {
    invokeMock.mockResolvedValueOnce(DEFAULT_SETTINGS);
    const { result } = renderHook(() => useSettings());
    await waitFor(() => expect(result.current.loaded).toBe(true));

    const next = { ...DEFAULT_SETTINGS, theme: "light" as const, annotationAuthor: "  Alice  " };
    const saved = { ...next, annotationAuthor: "Alice" };
    invokeMock.mockResolvedValueOnce({ settings: saved, association: null });

    let updateResult: Awaited<ReturnType<typeof result.current.update>> | undefined;
    await act(async () => {
      updateResult = await result.current.update(next);
    });

    expect(invokeMock).toHaveBeenCalledWith("update_settings", { settings: next });
    expect(updateResult).toEqual({ settings: saved, association: null });
    await waitFor(() => expect(result.current.settings.annotationAuthor).toBe("Alice"));
    expect(document.documentElement.dataset.theme).toBe("light");
  });
});
