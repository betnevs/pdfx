import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { AttachmentView, EditHistoryView } from "../lib/backend";
import { AttachmentPanel } from "./AttachmentPanel";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

const invokeMock = vi.mocked(invoke);

const text: AttachmentView = {
  id: 1,
  name: "report.txt",
  size: 17,
  mediaType: "text/plain",
};

const spreadsheet: AttachmentView = {
  id: 2,
  name: "data.csv",
  size: 2048,
  mediaType: "text/csv",
};

const dirtyHistory: EditHistoryView = {
  canUndo: true,
  canRedo: false,
  revision: 1,
  savedRevision: 0,
  dirty: true,
};

function renderPanel(
  overrides: Partial<{
    attachments: AttachmentView[];
    loading: boolean;
    error: string | null;
    canModify: boolean;
    readOnly: boolean;
    onHistory: (history: EditHistoryView) => void;
    onChanged: () => void;
  }> = {},
) {
  const onHistory = overrides.onHistory ?? vi.fn();
  const onChanged = overrides.onChanged ?? vi.fn();
  render(
    <AttachmentPanel
      documentId="doc-1"
      attachments={overrides.attachments ?? [text, spreadsheet]}
      loading={overrides.loading ?? false}
      error={overrides.error ?? null}
      canModify={overrides.canModify ?? true}
      readOnly={overrides.readOnly ?? false}
      onHistory={onHistory}
      onChanged={onChanged}
    />,
  );
  return { onHistory, onChanged };
}

describe("AttachmentPanel", () => {
  beforeEach(() => {
    invokeMock.mockResolvedValue(undefined);
  });

  afterEach(() => {
    invokeMock.mockReset();
    vi.mocked(openDialog).mockReset();
    vi.mocked(saveDialog).mockReset();
  });

  it("lists each attachment with its name, formatted size and type", () => {
    renderPanel();

    expect(screen.getByRole("heading", { name: "附件" })).toBeInTheDocument();
    expect(screen.getByText("report.txt")).toBeInTheDocument();
    expect(screen.getByText("17 B")).toBeInTheDocument();
    expect(screen.getByText("text/plain")).toBeInTheDocument();
    expect(screen.getByText("data.csv")).toBeInTheDocument();
    expect(screen.getByText("2.0 KB")).toBeInTheDocument();
    expect(screen.getByText("text/csv")).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "附件列表" })).toBeInTheDocument();
  });

  it("shows loading and empty states", () => {
    renderPanel({ attachments: [], loading: true });
    expect(screen.getByRole("status")).toHaveTextContent("正在载入附件…");
    expect(screen.queryByText("还没有附件")).not.toBeInTheDocument();
  });

  it("shows the empty list without a document attachment", () => {
    renderPanel({ attachments: [] });
    expect(screen.getByText("还没有附件")).toBeInTheDocument();
  });

  it("adds the file chosen in the native picker and publishes its history", async () => {
    const added: AttachmentView = {
      id: 3,
      name: "budget.xlsx",
      size: 4096,
      mediaType:
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    };
    vi.mocked(openDialog).mockResolvedValue("/tmp/budget.xlsx");
    invokeMock.mockResolvedValue({
      attachments: [text, spreadsheet, added],
      history: dirtyHistory,
    });
    const { onHistory, onChanged } = renderPanel();

    fireEvent.click(screen.getByRole("button", { name: "添加附件" }));

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("add_attachment", {
        documentId: "doc-1",
        path: "/tmp/budget.xlsx",
      }),
    );
    expect(onHistory).toHaveBeenCalledWith(dirtyHistory);
    expect(onChanged).toHaveBeenCalled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("does nothing when the attachment picker is cancelled", async () => {
    vi.mocked(openDialog).mockResolvedValue(null);
    const { onHistory, onChanged } = renderPanel();

    fireEvent.click(screen.getByRole("button", { name: "添加附件" }));

    await waitFor(() => expect(openDialog).toHaveBeenCalled());
    expect(invokeMock).not.toHaveBeenCalled();
    expect(onHistory).not.toHaveBeenCalled();
    expect(onChanged).not.toHaveBeenCalled();
  });

  it("deletes only after confirmation and publishes the new history", async () => {
    invokeMock.mockResolvedValue({
      attachments: [spreadsheet],
      history: dirtyHistory,
    });
    const { onHistory, onChanged } = renderPanel();

    fireEvent.click(screen.getByRole("button", { name: "删除 report.txt" }));
    const dialog = screen.getByRole("dialog", { name: "删除附件？" });
    expect(invokeMock).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "删除 report.txt" }));
    fireEvent.click(
      within(screen.getByRole("dialog", { name: "删除附件？" })).getByRole(
        "button",
        { name: "删除" },
      ),
    );

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("delete_attachment", {
        documentId: "doc-1",
        attachmentId: 1,
      }),
    );
    expect(onHistory).toHaveBeenCalledWith(dirtyHistory);
    expect(onChanged).toHaveBeenCalled();
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
  });

  it("opens an attachment only after the unsafe-file confirmation", async () => {
    renderPanel();

    fireEvent.click(screen.getByRole("button", { name: "打开 report.txt" }));
    const dialog = screen.getByRole("dialog", { name: "打开附件？" });
    expect(dialog).toHaveTextContent("附件可能包含恶意内容或病毒");
    expect(invokeMock).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "取消" }));
    expect(invokeMock).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "打开 report.txt" }));
    fireEvent.click(
      within(screen.getByRole("dialog", { name: "打开附件？" })).getByRole(
        "button",
        { name: "打开" },
      ),
    );

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("open_attachment", {
        documentId: "doc-1",
        attachmentId: 1,
        confirm: true,
      }),
    );
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
  });

  it("saves an attachment through the native save dialog", async () => {
    vi.mocked(saveDialog).mockResolvedValue("/tmp/report-copy.txt");
    invokeMock.mockResolvedValue({
      path: "/tmp/report-copy.txt",
      bytesWritten: 17,
    });
    renderPanel();

    fireEvent.click(screen.getByRole("button", { name: "另存为 report.txt" }));

    await waitFor(() =>
      expect(saveDialog).toHaveBeenCalledWith({
        defaultPath: "report.txt",
        title: "另存附件",
      }),
    );
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("save_attachment_as", {
        documentId: "doc-1",
        attachmentId: 1,
        destination: "/tmp/report-copy.txt",
      }),
    );
    expect(
      await screen.findByText("已另存到 /tmp/report-copy.txt"),
    ).toBeInTheDocument();
  });

  it("offers only the base name of a hostile attachment to the save dialog", async () => {
    const hostile: AttachmentView = {
      id: 3,
      name: "../../outside.txt",
      size: 4,
      mediaType: "text/plain",
    };
    vi.mocked(saveDialog).mockResolvedValue(null);
    renderPanel({ attachments: [hostile] });

    fireEvent.click(screen.getByRole("button", { name: "另存为 ../../outside.txt" }));

    await waitFor(() =>
      expect(saveDialog).toHaveBeenCalledWith({
        defaultPath: "outside.txt",
        title: "另存附件",
      }),
    );
  });

  it("does nothing when the attachment save dialog is cancelled", async () => {
    vi.mocked(saveDialog).mockResolvedValue(null);
    renderPanel();

    fireEvent.click(screen.getByRole("button", { name: "另存为 report.txt" }));

    await waitFor(() => expect(saveDialog).toHaveBeenCalled());
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("disables adding and deleting when the document cannot be modified", () => {
    renderPanel({ canModify: false });

    expect(screen.getByRole("button", { name: "添加附件" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "删除 report.txt" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "打开 report.txt" })).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "另存为 report.txt" }),
    ).toBeEnabled();
    expect(
      screen.getByText("当前文档的权限不允许添加或删除附件。"),
    ).toBeInTheDocument();
  });

  it("explains the read-only reason", () => {
    renderPanel({ canModify: false, readOnly: true });
    expect(
      screen.getByText("文档为只读，无法添加或删除附件。"),
    ).toBeInTheDocument();
  });

  it("shows Chinese messages for attachment failures", async () => {
    vi.mocked(openDialog).mockResolvedValue("/tmp/huge.bin");
    invokeMock.mockRejectedValue({
      code: "attachment_too_large",
      message: "file is larger than 100 MiB",
    });
    renderPanel();

    fireEvent.click(screen.getByRole("button", { name: "添加附件" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "附件过大，单个附件不能超过 100 MB。",
    );
  });

  it("shows a Chinese message when the save target is the source document", async () => {
    vi.mocked(saveDialog).mockResolvedValue("/tmp/report.pdf");
    invokeMock.mockRejectedValue({
      code: "attachment_target_is_source",
      message: "target is the Source Document",
    });
    renderPanel();

    fireEvent.click(screen.getByRole("button", { name: "另存为 report.txt" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "不能覆盖源文档，请选择其他位置。",
    );
  });

  it("shows the list load error the workspace passed in", () => {
    renderPanel({ attachments: [], error: "附件列表暂时不可用。" });
    expect(screen.getByRole("alert")).toHaveTextContent("附件列表暂时不可用。");
  });
});
