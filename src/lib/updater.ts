import { invoke } from "@tauri-apps/api/core";

/**
 * The JSON manifest published at the user-configured address:
 * `{ "version": "x.y.z", "notes": "...", "downloadUrl": "https://..." }`.
 * `notes` may be missing.
 */
export interface UpdateManifest {
  version: string;
  notes?: string | null;
  downloadUrl: string;
}

/** Whether the manifest describes a newer release. */
export type UpdateCheckStatus = "update_available" | "up_to_date";

/** The result of a successful `check_for_updates` call. */
export interface UpdateCheckResult {
  status: UpdateCheckStatus;
  currentVersion: string;
  version: string;
  notes: string | null;
  downloadUrl: string;
}

/** A structured error crossing the Tauri command boundary. */
interface CommandFailure {
  code: string;
  message?: string;
}

/**
 * Performs the only network request in PDFX.
 *
 * Called exclusively from the user's click on "检查更新"; the backend reads
 * the persisted manifest address, makes one GET with a finite timeout and no
 * retries, and returns the parsed result. No document or usage data is sent.
 */
export async function checkForUpdates(): Promise<UpdateCheckResult> {
  return invoke<UpdateCheckResult>("check_for_updates");
}

/** Opens the manifest's download page in the system browser. */
export async function openDownloadPage(url: string): Promise<void> {
  await invoke<void>("open_download_page", { url });
}

function isCommandFailure(error: unknown): error is CommandFailure {
  return (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    typeof (error as { code: unknown }).code === "string"
  );
}

/**
 * Maps a failed update command to copy the settings panel can show.
 *
 * `update_check_failed` is retryable and says so; the panel never retries on
 * its own, the user clicks "检查更新" again.
 */
export function describeUpdateError(error: unknown): string {
  if (isCommandFailure(error)) {
    const detail = error.message ?? "";
    switch (error.code) {
      case "update_not_configured":
        return "未配置更新地址。";
      case "update_check_failed": {
        const reason = detail.replace(/[。.!！]\s*$/, "");
        return reason.length > 0
          ? `检查更新失败：${reason}。请稍后重试。`
          : "检查更新失败，请稍后重试。";
      }
      case "update_manifest_invalid":
        return `更新信息格式不正确：${detail}`;
      case "update_open_failed":
        return `无法打开下载页：${detail}`;
      case "invalid_argument":
        return `更新地址无效：${detail}`;
      default:
        return detail;
    }
  }
  return error instanceof Error ? error.message : String(error);
}
