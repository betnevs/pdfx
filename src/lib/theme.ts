import type { Theme } from "./settings";

/** What the operating system (or a pinned choice) actually asks us to paint. */
export type ResolvedTheme = "light" | "dark";

export const DARK_SCHEME_QUERY = "(prefers-color-scheme: dark)";

/**
 * Best-effort mirror of the persisted choice, read synchronously before the
 * settings IPC resolves so a pinned theme is painted without a flash. The
 * backend remains the source of truth; `useSettings` applies its result as
 * soon as it loads.
 */
export const THEME_STORAGE_KEY = "pdfx.theme";

function isTheme(value: unknown): value is Theme {
  return value === "system" || value === "light" || value === "dark";
}

/**
 * `system` follows the OS; `light` and `dark` pin the interface. The PDF page
 * itself is never themed — only this resolution decides UI chrome.
 */
export function resolveTheme(theme: Theme, systemPrefersDark: boolean): ResolvedTheme {
  if (theme === "dark") {
    return "dark";
  }
  if (theme === "light") {
    return "light";
  }
  return systemPrefersDark ? "dark" : "light";
}

function darkSchemeQuery(): MediaQueryList | null {
  try {
    return typeof window.matchMedia === "function"
      ? window.matchMedia(DARK_SCHEME_QUERY)
      : null;
  } catch {
    return null;
  }
}

/** True when the operating system currently asks for a dark color scheme. */
export function systemPrefersDark(): boolean {
  return darkSchemeQuery()?.matches ?? false;
}

/**
 * Calls `onChange` whenever the OS switches between light and dark.
 *
 * CSS `prefers-color-scheme` already repaints the interface on its own; this
 * subscription keeps the resolved theme observable for the rest of the app
 * (and for tests). Returns an unsubscribe function.
 */
export function watchSystemTheme(onChange: (prefersDark: boolean) => void): () => void {
  const query = darkSchemeQuery();
  if (!query) {
    return () => {};
  }

  const listener = (event: MediaQueryListEvent) => onChange(event.matches);
  query.addEventListener("change", listener);
  return () => query.removeEventListener("change", listener);
}

/** Remembers the choice for the next start-up. Storage can be unavailable. */
export function cacheTheme(theme: Theme): void {
  try {
    window.localStorage.setItem(THEME_STORAGE_KEY, theme);
  } catch {
    // Private browsing or blocked storage: the backend still persists it.
  }
}

/** The last successfully cached choice, or null when absent or invalid. */
export function readCachedTheme(): Theme | null {
  try {
    const value = window.localStorage.getItem(THEME_STORAGE_KEY);
    return isTheme(value) ? value : null;
  } catch {
    return null;
  }
}

/**
 * Applies a theme choice to `<html>`.
 *
 * `system` removes `data-theme` so the CSS `prefers-color-scheme` rules take
 * over immediately when the OS flips; a pinned choice sets the attribute and
 * always wins. `data-resolved-theme` records the concrete light/dark result
 * for the rest of the app.
 */
export function applyTheme(theme: Theme): ResolvedTheme {
  const root = document.documentElement;
  const resolved = resolveTheme(theme, systemPrefersDark());

  if (theme === "system") {
    root.removeAttribute("data-theme");
  } else {
    root.dataset.theme = theme;
  }
  root.dataset.resolvedTheme = resolved;
  cacheTheme(theme);

  return resolved;
}

/** Re-resolves the theme after an OS change without changing the choice. */
export function refreshResolvedTheme(theme: Theme): ResolvedTheme {
  const resolved = resolveTheme(theme, systemPrefersDark());
  document.documentElement.dataset.resolvedTheme = resolved;
  return resolved;
}

/**
 * The choice currently applied to the document.
 *
 * Settings may be temporarily unavailable (for example outside the Tauri
 * shell), so the applied attribute — not the last loaded settings object — is
 * the source of truth for OS-change bookkeeping.
 */
export function appliedTheme(): Theme {
  const attribute = document.documentElement.dataset.theme;
  return attribute === "light" || attribute === "dark" ? attribute : "system";
}

/**
 * Paints the cached choice before React renders, so start-up never flashes a
 * light UI for a user who pinned dark. Without a cache the app follows the
 * system, which the CSS media query already handles before JavaScript runs.
 */
export function initializeTheme(): ResolvedTheme {
  return applyTheme(readCachedTheme() ?? "system");
}
