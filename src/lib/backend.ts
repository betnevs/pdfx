import { invoke } from "@tauri-apps/api/core";

/** Application identity exposed by the Rust command boundary. */
export interface AppInfo {
  name: string;
  version: string;
}

/**
 * Reads the application identity through the single Tauri command seam.
 *
 * Every frontend feature talks to the Rust backend through this module, so
 * tests can replace the IPC boundary without knowing about PDF internals.
 */
export async function getAppInfo(): Promise<AppInfo> {
  return invoke<AppInfo>("app_info");
}
