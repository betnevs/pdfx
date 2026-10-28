import { useCallback, useEffect, useState } from "react";

import {
  applySettings,
  DEFAULT_SETTINGS,
  getSettings,
  updateSettings,
  type Settings,
  type SettingsUpdateResult,
} from "./settings";

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

  const update = useCallback(async (next: Settings): Promise<SettingsUpdateResult> => {
    const result = await updateSettings(next);
    setSettings(result.settings);
    applySettings(result.settings);
    return result;
  }, []);

  return { settings, loaded, update };
}
