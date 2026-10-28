import { afterEach, describe, expect, it, vi } from "vitest";

import {
  applyTheme,
  initializeTheme,
  readCachedTheme,
  refreshResolvedTheme,
  resolveTheme,
  THEME_STORAGE_KEY,
  watchSystemTheme,
} from "./theme";

interface FakeMediaQueryList {
  matches: boolean;
  media: string;
  addEventListener: (
    type: "change",
    listener: (event: MediaQueryListEvent) => void,
  ) => void;
  removeEventListener: (
    type: "change",
    listener: (event: MediaQueryListEvent) => void,
  ) => void;
  dispatch: (matches: boolean) => void;
  listenerCount: () => number;
}

/** Installs a controllable `prefers-color-scheme` query. */
function installColorScheme(initiallyDark: boolean): FakeMediaQueryList {
  const listeners = new Set<(event: MediaQueryListEvent) => void>();
  const query: FakeMediaQueryList = {
    matches: initiallyDark,
    media: "(prefers-color-scheme: dark)",
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
    listenerCount: () => listeners.size,
  };
  vi.stubGlobal("matchMedia", vi.fn(() => query));
  return query;
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  document.documentElement.removeAttribute("data-theme");
  document.documentElement.removeAttribute("data-resolved-theme");
  window.localStorage.clear();
});

describe("resolveTheme", () => {
  it("pins an explicit light or dark choice regardless of the operating system", () => {
    expect(resolveTheme("light", false)).toBe("light");
    expect(resolveTheme("light", true)).toBe("light");
    expect(resolveTheme("dark", false)).toBe("dark");
    expect(resolveTheme("dark", true)).toBe("dark");
  });

  it("follows the operating system when the theme is system", () => {
    expect(resolveTheme("system", false)).toBe("light");
    expect(resolveTheme("system", true)).toBe("dark");
  });
});

describe("watchSystemTheme", () => {
  it("reports operating-system changes until it is unsubscribed", () => {
    const query = installColorScheme(false);
    const onChange = vi.fn<() => void>();

    const stop = watchSystemTheme(onChange);
    expect(query.listenerCount()).toBe(1);

    query.dispatch(true);
    expect(onChange).toHaveBeenLastCalledWith(true);

    query.dispatch(false);
    expect(onChange).toHaveBeenLastCalledWith(false);
    expect(onChange).toHaveBeenCalledTimes(2);

    stop();
    expect(query.listenerCount()).toBe(0);
    query.dispatch(true);
    expect(onChange).toHaveBeenCalledTimes(2);
  });

  it("returns a no-op unsubscribe where matchMedia is unavailable", () => {
    const stop = watchSystemTheme(vi.fn());

    expect(typeof stop).toBe("function");
    expect(() => stop()).not.toThrow();
  });
});

describe("applying the theme", () => {
  it("pins a dark choice on the document element and caches it for the next start", () => {
    installColorScheme(false);

    expect(applyTheme("dark")).toBe("dark");

    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.dataset.resolvedTheme).toBe("dark");
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBe("dark");
  });

  it("leaves the attribute off for system so the media query follows the OS", () => {
    installColorScheme(true);

    expect(applyTheme("system")).toBe("dark");

    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
    expect(document.documentElement.dataset.resolvedTheme).toBe("dark");
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBe("system");
  });

  it("keeps an explicit light choice while the OS is dark", () => {
    installColorScheme(true);

    expect(applyTheme("light")).toBe("light");

    expect(document.documentElement.dataset.theme).toBe("light");
    expect(document.documentElement.dataset.resolvedTheme).toBe("light");
  });

  it("keeps working when local storage is unavailable", () => {
    installColorScheme(false);
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("storage is blocked");
    });

    expect(() => applyTheme("dark")).not.toThrow();
    expect(document.documentElement.dataset.theme).toBe("dark");
  });
});

describe("cached theme and start-up", () => {
  it("paints a cached pinned theme before settings load", () => {
    installColorScheme(false);
    window.localStorage.setItem(THEME_STORAGE_KEY, "dark");

    expect(initializeTheme()).toBe("dark");

    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.dataset.resolvedTheme).toBe("dark");
  });

  it("falls back to following the system when no valid theme is cached", () => {
    installColorScheme(true);
    window.localStorage.setItem(THEME_STORAGE_KEY, "sepia");

    expect(initializeTheme()).toBe("dark");

    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
    expect(document.documentElement.dataset.resolvedTheme).toBe("dark");
    expect(readCachedTheme()).toBe("system");
  });

  it("only updates the resolved theme after an OS change while following the system", () => {
    installColorScheme(false);
    applyTheme("system");

    installColorScheme(true);
    expect(refreshResolvedTheme("system")).toBe("dark");

    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
    expect(document.documentElement.dataset.resolvedTheme).toBe("dark");
  });
});
