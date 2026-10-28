import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  checkForUpdates,
  describeUpdateError,
  openDownloadPage,
  type UpdateCheckResult,
} from "./updater";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
});

describe("update command seam", () => {
  it("checks for updates through the Tauri command name without arguments", async () => {
    const result: UpdateCheckResult = {
      status: "update_available",
      currentVersion: "0.1.0",
      version: "0.2.0",
      notes: "新增批量导出。",
      downloadUrl: "https://example.com/pdfx/download",
    };
    invokeMock.mockResolvedValueOnce(result);

    await expect(checkForUpdates()).resolves.toEqual(result);
    expect(invokeMock).toHaveBeenCalledWith("check_for_updates");
    expect(invokeMock.mock.calls[0]).toHaveLength(1);
  });

  it("opens the download page through the system opener command", async () => {
    invokeMock.mockResolvedValueOnce(undefined);

    await openDownloadPage("https://example.com/pdfx/download");

    expect(invokeMock).toHaveBeenCalledWith("open_download_page", {
      url: "https://example.com/pdfx/download",
    });
  });
});

describe("describeUpdateError", () => {
  it("turns command errors into retryable localized copy", () => {
    expect(
      describeUpdateError({
        code: "update_check_failed",
        message: "the update check timed out",
      }),
    ).toBe("检查更新失败：the update check timed out。请稍后重试。");

    expect(
      describeUpdateError({ code: "update_check_failed", message: "更新检查超时。" }),
    ).toBe("检查更新失败：更新检查超时。请稍后重试。");

    expect(describeUpdateError({ code: "update_check_failed" })).toBe(
      "检查更新失败，请稍后重试。",
    );

    expect(describeUpdateError({ code: "update_not_configured", message: "未配置更新地址。" })).toBe(
      "未配置更新地址。",
    );

    expect(
      describeUpdateError({
        code: "update_manifest_invalid",
        message: "could not parse the version manifest",
      }),
    ).toContain("更新信息格式不正确");

    expect(describeUpdateError({ code: "update_open_failed", message: "no browser" })).toBe(
      "无法打开下载页：no browser",
    );

    expect(describeUpdateError({ code: "invalid_argument", message: "更新地址只支持 http" })).toBe(
      "更新地址无效：更新地址只支持 http",
    );
  });

  it("falls back to the error text for values that are not command errors", () => {
    expect(describeUpdateError(new Error("offline"))).toBe("offline");
    expect(describeUpdateError("boom")).toBe("boom");
  });
});
