import { invoke } from "@tauri-apps/api/core";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  applyLanguage,
  applySettings,
  applyTheme,
  chooseDiagnosticsDestination,
  DEFAULT_SETTINGS,
  exportDiagnostics,
  getSettings,
  updateSettings,
} from "./settings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const saveMock = vi.mocked(saveDialog);

beforeEach(() => {
  invokeMock.mockResolvedValue(undefined);
});

afterEach(() => {
  invokeMock.mockReset();
  saveMock.mockReset();
  document.documentElement.removeAttribute("data-theme");
  document.documentElement.lang = "";
});

describe("settings command seam", () => {
  it("loads and updates settings through the Tauri command names", async () => {
    const stored = { ...DEFAULT_SETTINGS, theme: "dark" as const };
    invokeMock.mockResolvedValueOnce(stored);

    await expect(getSettings()).resolves.toEqual(stored);
    expect(invokeMock).toHaveBeenCalledWith("get_settings");

    invokeMock.mockResolvedValueOnce({ settings: stored, association: null });
    const result = await updateSettings(stored);
    expect(result.settings).toEqual(stored);
    expect(result.association).toBeNull();
    expect(invokeMock).toHaveBeenCalledWith("update_settings", { settings: stored });
  });

  it("exports diagnostics to the chosen path", async () => {
    const exported = { path: "/tmp/pdfx-diagnostics.txt", bytes: 42, logFiles: 2 };
    invokeMock.mockResolvedValueOnce(exported);

    await expect(exportDiagnostics("/tmp/pdfx-diagnostics.txt")).resolves.toEqual(exported);
    expect(invokeMock).toHaveBeenCalledWith("export_diagnostics", {
      destination: "/tmp/pdfx-diagnostics.txt",
    });
  });

  it("asks for a text-file destination and handles cancellation", async () => {
    saveMock.mockResolvedValueOnce("/tmp/pdfx-diagnostics.txt");

    await expect(chooseDiagnosticsDestination()).resolves.toBe("/tmp/pdfx-diagnostics.txt");
    expect(saveMock).toHaveBeenCalledWith(
      expect.objectContaining({
        defaultPath: "pdfx-diagnostics.txt",
        filters: [{ name: "文本", extensions: ["txt"] }],
      }),
    );

    saveMock.mockResolvedValueOnce(null);
    await expect(chooseDiagnosticsDestination()).resolves.toBeNull();
  });

  it("applies explicit themes and clears the attribute for system", () => {
    applyTheme("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");

    applyTheme("light");
    expect(document.documentElement.dataset.theme).toBe("light");

    applyTheme("system");
    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
  });

  it("applies the interface language to the document element", () => {
    applyLanguage("en-US");
    expect(document.documentElement.lang).toBe("en-US");

    applyLanguage("zh-CN");
    expect(document.documentElement.lang).toBe("zh-CN");
  });

  it("applies theme and language together", () => {
    applySettings({ theme: "dark", language: "en-US" });

    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.lang).toBe("en-US");
  });
});
