import { act, renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { DEFAULT_SETTINGS } from "./settings";
import { initializeTheme, THEME_STORAGE_KEY } from "./theme";
import { useSettings } from "./useSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

interface FakeMediaQueryList {
  matches: boolean;
  addEventListener: (
    type: "change",
    listener: (event: MediaQueryListEvent) => void,
  ) => void;
  removeEventListener: (
    type: "change",
    listener: (event: MediaQueryListEvent) => void,
  ) => void;
  dispatch: (matches: boolean) => void;
}

/** Installs a controllable `prefers-color-scheme` query. */
function installColorScheme(initiallyDark: boolean): FakeMediaQueryList {
  const listeners = new Set<(event: MediaQueryListEvent) => void>();
  const query: FakeMediaQueryList = {
    matches: initiallyDark,
    addEventListener: (_type, listener) => {
      listeners.add(listener);
    },
    removeEventListener: (_type, listener) => {
      listeners.delete(listener);
    },
    dispatch: (matches) => {
      query.matches = matches;
      for (const listener of [...listeners]) {
        listener({ matches } as MediaQueryListEvent);
      }
    },
  };
  vi.stubGlobal("matchMedia", vi.fn(() => query));
  return query;
}

beforeEach(() => {
  invokeMock.mockReset();
});

afterEach(() => {
  vi.unstubAllGlobals();
  document.documentElement.removeAttribute("data-theme");
  document.documentElement.removeAttribute("data-resolved-theme");
  document.documentElement.lang = "";
  window.localStorage.clear();
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

  it("follows OS color-scheme changes immediately while the theme is system", async () => {
    const query = installColorScheme(false);
    invokeMock.mockResolvedValueOnce({ ...DEFAULT_SETTINGS, theme: "system" });
    const { result } = renderHook(() => useSettings());
    await waitFor(() => expect(result.current.loaded).toBe(true));

    expect(document.documentElement.dataset.resolvedTheme).toBe("light");
    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);

    act(() => query.dispatch(true));

    expect(document.documentElement.dataset.resolvedTheme).toBe("dark");
    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);

    act(() => query.dispatch(false));

    expect(document.documentElement.dataset.resolvedTheme).toBe("light");
  });

  it("keeps the cached pinned theme when settings cannot be loaded", async () => {
    const query = installColorScheme(false);
    window.localStorage.setItem(THEME_STORAGE_KEY, "dark");
    initializeTheme();
    invokeMock.mockRejectedValueOnce(new Error("not running inside Tauri"));

    const { result } = renderHook(() => useSettings());
    await waitFor(() => expect(result.current.loaded).toBe(true));

    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.dataset.resolvedTheme).toBe("dark");

    act(() => query.dispatch(true));
    act(() => query.dispatch(false));

    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.dataset.resolvedTheme).toBe("dark");
  });

  it("ignores OS changes when the user pinned a theme", async () => {
    const query = installColorScheme(false);
    invokeMock.mockResolvedValueOnce({ ...DEFAULT_SETTINGS, theme: "light" });
    const { result } = renderHook(() => useSettings());
    await waitFor(() => expect(result.current.loaded).toBe(true));

    act(() => query.dispatch(true));

    expect(document.documentElement.dataset.theme).toBe("light");
    expect(document.documentElement.dataset.resolvedTheme).toBe("light");
  });
});
