import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { SettingsPanel } from "./SettingsPanel";
import type { Settings, SettingsUpdateResult } from "../lib/settings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn() }));

const settings: Settings = {
  language: "zh-CN",
  theme: "system",
  annotationAuthor: "PDFX 用户",
  fileAssociationEnabled: false,
};

const invokeMock = vi.mocked(invoke);
const saveMock = vi.mocked(saveDialog);

function createOnSave() {
  return vi.fn(
    async (next: Settings): Promise<SettingsUpdateResult> => ({
      settings: next,
      association: null,
    }),
  );
}

function renderPanel(
  options: {
    onSave?: ReturnType<typeof createOnSave>;
    onClose?: () => void;
    open?: boolean;
    current?: Settings;
  } = {},
) {
  const onSave = options.onSave ?? createOnSave();
  const onClose = options.onClose ?? vi.fn();
  render(
    <SettingsPanel
      open={options.open ?? true}
      settings={options.current ?? settings}
      onClose={onClose}
      onSave={onSave}
    />,
  );
  return { onSave, onClose };
}

beforeEach(() => {
  invokeMock.mockReset();
  saveMock.mockReset();
});

afterEach(() => {
  invokeMock.mockReset();
  saveMock.mockReset();
});

describe("SettingsPanel", () => {
  it("renders every settings control and no telemetry or auto-upload option", () => {
    renderPanel();

    expect(screen.getByRole("dialog", { name: "设置" })).toBeInTheDocument();
    expect(screen.getByLabelText("界面语言")).toBeInTheDocument();
    expect(screen.getByLabelText("主题")).toBeInTheDocument();
    expect(screen.getByLabelText("Annotation 显示名")).toBeInTheDocument();
    expect(
      screen.getByRole("checkbox", { name: "将 PDFX 注册为 PDF 打开方式" }),
    ).not.toBeChecked();
    expect(screen.getByRole("heading", { name: "更新检查" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "检查更新" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "导出诊断包…" })).toBeInTheDocument();

    expect(screen.getAllByRole("checkbox")).toHaveLength(1);
    expect(screen.queryByText(/遥测/)).not.toBeInTheDocument();
  });

  it("commits theme changes immediately", async () => {
    const { onSave } = renderPanel();
    const user = userEvent.setup();

    await user.selectOptions(screen.getByLabelText("主题"), "dark");

    await waitFor(() =>
      expect(onSave).toHaveBeenCalledWith({ ...settings, theme: "dark" }),
    );
    expect(await screen.findByRole("status")).toHaveTextContent("已保存");
  });

  it("commits language changes immediately", async () => {
    const { onSave } = renderPanel();
    const user = userEvent.setup();

    await user.selectOptions(screen.getByLabelText("界面语言"), "en-US");

    await waitFor(() =>
      expect(onSave).toHaveBeenCalledWith({ ...settings, language: "en-US" }),
    );
  });

  it("commits the annotation author on blur and trims it", async () => {
    const { onSave } = renderPanel();
    const user = userEvent.setup();
    const input = screen.getByLabelText("Annotation 显示名");

    await user.clear(input);
    await user.type(input, "  Ada Lovelace  ");
    await user.tab();

    await waitFor(() =>
      expect(onSave).toHaveBeenCalledWith({
        ...settings,
        annotationAuthor: "Ada Lovelace",
      }),
    );
  });

  it("rejects an empty annotation author without calling the backend", async () => {
    const { onSave } = renderPanel();
    const user = userEvent.setup();
    const input = screen.getByLabelText("Annotation 显示名");

    await user.clear(input);
    await user.tab();

    expect(await screen.findByRole("alert")).toHaveTextContent("不能为空");
    expect(onSave).not.toHaveBeenCalled();
  });

  it("shows the operating system's guidance when the user must confirm the default app", async () => {
    const onSave = vi.fn(
      async (next: Settings): Promise<SettingsUpdateResult> => ({
        settings: next,
        association: {
          status: "user_action_required",
          instructions: "请打开 设置 → 应用 → 默认应用 并选择 PDFX。",
          message: null,
        },
      }),
    );
    renderPanel({ onSave });
    const user = userEvent.setup();

    await user.click(
      screen.getByRole("checkbox", { name: "将 PDFX 注册为 PDF 打开方式" }),
    );

    expect(
      await screen.findByText("请打开 设置 → 应用 → 默认应用 并选择 PDFX。"),
    ).toBeInTheDocument();
    expect(onSave).toHaveBeenCalledWith({
      ...settings,
      fileAssociationEnabled: true,
    });
  });

  it("keeps the last saved settings when persisting fails", async () => {
    const onSave = vi.fn(async (): Promise<SettingsUpdateResult> => {
      throw new Error("disk full");
    });
    renderPanel({ onSave });
    const user = userEvent.setup();

    await user.selectOptions(screen.getByLabelText("主题"), "dark");

    expect(await screen.findByRole("alert")).toHaveTextContent("保存失败");
    expect(await screen.findByRole("alert")).toHaveTextContent("disk full");
    expect(screen.getByLabelText("主题")).toHaveValue("system");
  });

  it("exports diagnostics to the path the user chooses", async () => {
    saveMock.mockResolvedValueOnce("/tmp/pdfx-diagnostics.txt");
    invokeMock.mockResolvedValueOnce({
      path: "/tmp/pdfx-diagnostics.txt",
      bytes: 120,
      logFiles: 2,
    });
    renderPanel();
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "导出诊断包…" }));

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("export_diagnostics", {
        destination: "/tmp/pdfx-diagnostics.txt",
      }),
    );
    expect(await screen.findByText(/诊断包已保存/)).toBeInTheDocument();
  });

  it("does not export when the save dialog is cancelled", async () => {
    saveMock.mockResolvedValueOnce(null);
    renderPanel();
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "导出诊断包…" }));

    await waitFor(() => expect(saveMock).toHaveBeenCalled());
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("closes from the close button and Escape", async () => {
    const onClose = vi.fn<() => void>();
    renderPanel({ onClose });
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "关闭设置" }));
    expect(onClose).toHaveBeenCalledTimes(1);

    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("renders nothing while closed", () => {
    renderPanel({ open: false });

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});
