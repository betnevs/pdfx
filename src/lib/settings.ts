import { invoke } from "@tauri-apps/api/core";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";

import { applyTheme } from "./theme";

/** Interface language options offered by the settings page. */
export type InterfaceLanguage = "zh-CN" | "en-US";

/** `system` follows the operating system; the others pin the theme. */
export type Theme = "system" | "light" | "dark";

/**
 * The settings the backend persists under the app config directory.
 * `annotationAuthor` is the author later annotation tickets use.
 */
export interface Settings {
  language: InterfaceLanguage;
  theme: Theme;
  annotationAuthor: string;
  fileAssociationEnabled: boolean;
  /**
   * Address of the version manifest the user-initiated update check reads.
   * Empty means the check is not configured and no request is made.
   */
  updateManifestUrl: string;
}

/**
 * Mirrors `Settings::default()` in `src-tauri/src/settings.rs`. It is used
 * while settings load and when the frontend runs without the Tauri shell.
 */
export const DEFAULT_SETTINGS: Settings = {
  language: "zh-CN",
  theme: "system",
  annotationAuthor: "PDFX 用户",
  fileAssociationEnabled: false,
  updateManifestUrl: "",
};

/**
 * How far the OS let the file-association attempt get.
 *
 * `user_action_required` is the normal outcome on modern macOS and Windows:
 * PDFX is registered as an available handler, but the user must confirm it in
 * the OS default-apps UI.
 */
export type AssociationStatus =
  | "registered"
  | "user_action_required"
  | "unregistered"
  | "unsupported"
  | "failed";

export interface AssociationResult {
  status: AssociationStatus;
  instructions: string;
  message: string | null;
}

export interface SettingsUpdateResult {
  settings: Settings;
  association: AssociationResult | null;
}

export interface DiagnosticsExport {
  path: string;
  bytes: number;
  logFiles: number;
}

/** Reads the persisted settings; missing or corrupt settings return defaults. */
export async function getSettings(): Promise<Settings> {
  return invoke<Settings>("get_settings");
}

/** Persists settings and applies file-association changes on the OS side. */
export async function updateSettings(settings: Settings): Promise<SettingsUpdateResult> {
  return invoke<SettingsUpdateResult>("update_settings", { settings });
}

/** Writes diagnostics to an explicit path chosen by the user. */
export async function exportDiagnostics(destination: string): Promise<DiagnosticsExport> {
  return invoke<DiagnosticsExport>("export_diagnostics", { destination });
}

/** Asks the user where to save the diagnostic bundle. Returns null on cancel. */
export async function chooseDiagnosticsDestination(): Promise<string | null> {
  const selection = await saveDialog({
    title: "导出诊断包",
    defaultPath: "pdfx-diagnostics.txt",
    filters: [{ name: "文本", extensions: ["txt"] }],
  });
  return typeof selection === "string" ? selection : null;
}

/**
 * Applies the theme to the document. `system` removes the attribute so the
 * `prefers-color-scheme` styles take over.
 *
 * Re-exported from `theme.ts`, which also caches the choice before paint and
 * tracks the concrete resolved theme.
 */
export { applyTheme };

/** Records the interface language for assistive tech and future copy. */
export function applyLanguage(language: InterfaceLanguage): void {
  document.documentElement.lang = language;
}

export function applySettings(settings: Pick<Settings, "theme" | "language">): void {
  applyTheme(settings.theme);
  applyLanguage(settings.language);
}
