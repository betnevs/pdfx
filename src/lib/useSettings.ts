import { useCallback, useEffect, useState } from "react";

import {
  applySettings,
  DEFAULT_SETTINGS,
  getSettings,
  updateSettings,
  type Settings,
  type SettingsUpdateResult,
} from "./settings";
import { appliedTheme, refreshResolvedTheme, watchSystemTheme } from "./theme";

export interface SettingsController {
  /** The last settings the backend returned, or defaults while unavailable. */
  settings: Settings;
  /** True once loading finished, successfully or not. */
  loaded: boolean;
  /** Persists settings and applies the backend's normalized result. */
  update: (next: Settings) => Promise<SettingsUpdateResult>;
}

/**
 * Loads settings once at app start and applies theme and language immediately.
 *
 * Outside the Tauri shell the command is unavailable; the app keeps working
 * with defaults instead of failing.
 */
export function useSettings(): SettingsController {
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      try {
        const stored = await getSettings();
        if (cancelled) {
          return;
        }
        const merged = { ...DEFAULT_SETTINGS, ...stored };
        setSettings(merged);
        applySettings(merged);
      } catch {
        // Vite dev without Tauri, or a transient IPC failure: defaults apply.
      } finally {
        if (!cancelled) {
          setLoaded(true);
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  // Following the system is a CSS media-query decision; this keeps the
  // concrete resolved theme in sync so the rest of the app can observe the
  // live light/dark state without restarting. Re-resolve from the theme that
  // is actually applied: settings may not have loaded while a cached pinned
  // theme is already on screen.
  useEffect(() => {
    if (settings.theme !== "system") {
      return;
    }
    return watchSystemTheme(() => {
      refreshResolvedTheme(appliedTheme());
    });
  }, [settings.theme]);

  const update = useCallback(async (next: Settings): Promise<SettingsUpdateResult> => {
    const result = await updateSettings(next);
    setSettings(result.settings);
    applySettings(result.settings);
    return result;
  }, []);

  return { settings, loaded, update };
}
