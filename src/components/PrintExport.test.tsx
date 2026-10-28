import { createRef } from "react";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import type { DocumentPermissions, DocumentView } from "../lib/backend";
import { PRINT_DISABLED_REASON } from "../lib/document-policy";
import { PrintExport, type PrintExportHandle } from "./PrintExport";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const saveMock = vi.mocked(saveDialog);

// jsdom does not implement object URLs; the print surface uses them instead of
// embedding base64 in the DOM, so the tests record their lifecycle.
const createdObjectUrls: string[] = [];
const revokedObjectUrls: string[] = [];
let objectUrlCounter = 0;

beforeAll(() => {
  Object.assign(URL, {
    createObjectURL: (_blob: Blob) => {
      objectUrlCounter += 1;
      const url = `blob:pdfx-print-${objectUrlCounter}`;
      createdObjectUrls.push(url);
      return url;
    },
    revokeObjectURL: (url: string) => {
      revokedObjectUrls.push(url);
    },
  });
});

beforeEach(() => {
  objectUrlCounter = 0;
  createdObjectUrls.length = 0;
  revokedObjectUrls.length = 0;
});

const allGranted: DocumentPermissions = {
  canModifyContent: true,
  canAnnotate: true,
  canAssemble: true,
  canFillForms: true,
  canPrint: true,
  canCopyContent: true,
};

function documentView(overrides: Partial<DocumentView> = {}): DocumentView {
  return {
    documentId: "doc-1",
    name: "report.pdf",
    path: "/tmp/report.pdf",
    pageCount: 3,
    pageWidth: 612,
    pageHeight: 792,
    encrypted: false,
    readOnly: false,
    permissions: allGranted,
    ...overrides,
  };
}

function renderPrintExport(document = documentView()) {
  const ref = createRef<PrintExportHandle>();
  render(<PrintExport ref={ref} document={document} pageIndex={1} />);
  return ref;
}

afterEach(() => {
  vi.resetAllMocks();
  vi.unstubAllGlobals();
});

describe("PrintExport", () => {
  it("disables printing with a visible reason when Permission Restrictions deny it", async () => {
    const user = userEvent.setup();
    const ref = renderPrintExport(
      documentView({
        permissions: { ...allGranted, canPrint: false },
      }),
    );

    const print = screen.getByRole("button", { name: "打印…" });
    expect(print).toBeDisabled();
    expect(screen.getByText(PRINT_DISABLED_REASON)).toBeInTheDocument();

    await user.click(print);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    ref.current?.openPrintDialog();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("keeps export available when printing is denied", () => {
    renderPrintExport(
      documentView({
        permissions: { ...allGranted, canPrint: false },
      }),
    );

    expect(screen.getByRole("button", { name: "导出当前页…" })).toBeEnabled();
  });

  it("exports the current page with the chosen format and resolution", async () => {
    const user = userEvent.setup();
    saveMock.mockResolvedValue("/tmp/report-page-2.jpg");
    invokeMock.mockResolvedValue({
      path: "/tmp/report-page-2.jpg",
      format: "jpg",
      dpi: 300,
      width: 2550,
      height: 3300,
      bytes: 999,
    });
    renderPrintExport();

    await user.click(screen.getByRole("button", { name: "导出当前页…" }));
    const dialog = screen.getByRole("dialog", { name: "导出当前页" });
    expect(within(dialog).getByText(/第 2 页，共 3 页/)).toBeInTheDocument();

    await user.selectOptions(screen.getByLabelText("格式"), "jpg");
    await user.selectOptions(screen.getByLabelText("分辨率"), "300");
    await user.click(within(dialog).getByRole("button", { name: "导出…" }));

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("export_page", {
        documentId: "doc-1",
        pageIndex: 1,
        format: "jpg",
        dpi: 300,
        destination: "/tmp/report-page-2.jpg",
      }),
    );
    expect(saveMock).toHaveBeenCalledWith({
      defaultPath: "report-page-2.jpg",
      filters: [{ name: "JPG", extensions: ["jpg"] }],
    });
    expect(await screen.findByRole("status")).toHaveTextContent(
      "已导出第 2 页",
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual([
      "export_page",
    ]);
  });

  it("does not export when the save dialog is cancelled", async () => {
    const user = userEvent.setup();
    saveMock.mockResolvedValue(null);
    renderPrintExport();

    await user.click(screen.getByRole("button", { name: "导出当前页…" }));
    await user.click(
      within(screen.getByRole("dialog", { name: "导出当前页" })).getByRole(
        "button",
        { name: "导出…" },
      ),
    );

    await waitFor(() => expect(saveMock).toHaveBeenCalled());
    expect(invokeMock).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog", { name: "导出当前页" })).toBeInTheDocument();
  });

  it("shows a localized error and keeps the dialog open when writing fails", async () => {
    const user = userEvent.setup();
    saveMock.mockResolvedValue("/tmp/report-page-2.png");
    invokeMock.mockRejectedValue({
      code: "file_write_failed",
      message: "Permission denied (os error 13)",
    });
    renderPrintExport();

    await user.click(screen.getByRole("button", { name: "导出当前页…" }));
    await user.click(
      within(screen.getByRole("dialog", { name: "导出当前页" })).getByRole(
        "button",
        { name: "导出…" },
      ),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "无法写入文件，请检查目标位置与磁盘空间。",
    );
    expect(screen.getByRole("dialog", { name: "导出当前页" })).toBeInTheDocument();
  });

  it("explains why the source PDF cannot be used as the export target", async () => {
    const user = userEvent.setup();
    saveMock.mockResolvedValue("/tmp/report.pdf");
    invokeMock.mockRejectedValue({
      code: "export_target_is_source",
      message: "/tmp/report.pdf is the open Source Document",
    });
    renderPrintExport();

    await user.click(screen.getByRole("button", { name: "导出当前页…" }));
    await user.click(
      within(screen.getByRole("dialog", { name: "导出当前页" })).getByRole(
        "button",
        { name: "导出…" },
      ),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "不能覆盖源 PDF 文件，请选择其他位置。",
    );
  });

  it("explains why the current output PDF cannot be used as the export target", async () => {
    const user = userEvent.setup();
    saveMock.mockResolvedValue("/tmp/report-output.pdf");
    invokeMock.mockRejectedValue({
      code: "export_target_is_output",
      message: "/tmp/report-output.pdf is the current Output Document",
    });
    renderPrintExport();

    await user.click(screen.getByRole("button", { name: "导出当前页…" }));
    await user.click(
      within(screen.getByRole("dialog", { name: "导出当前页" })).getByRole(
        "button",
        { name: "导出…" },
      ),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "不能覆盖当前输出 PDF 文件，请选择其他位置。",
    );
  });
});

function flattenResult(overrides: Record<string, unknown> = {}) {
  return {
    documentId: "doc-1",
    path: "/tmp/report-flattened.pdf",
    bytes: 2048,
    flattened: 4,
    preserved: 2,
    skipped: [],
    ...overrides,
  };
}

async function openFlattenDialog() {
  const user = userEvent.setup();
  renderPrintExport();
  await user.click(
    screen.getByRole("button", { name: "导出 Flatten 副本…" }),
  );
  return {
    user,
    dialog: screen.getByRole("dialog", { name: "导出 Flatten 副本" }),
  };
}

describe("PrintExport flattened copy", () => {
  it("exports a flattened copy and reports the counts in Chinese", async () => {
    saveMock.mockResolvedValue("/tmp/report-flattened.pdf");
    invokeMock.mockResolvedValue(
      flattenResult({
        skipped: [
          { pageIndex: 0, subtype: "Square", reason: "no_appearance" },
          { pageIndex: 1, subtype: "Text", reason: "popup" },
        ],
      }),
    );
    const { user, dialog } = await openFlattenDialog();

    expect(within(dialog).getByText(/视觉标注将绘制为页面内容/)).toBeInTheDocument();
    expect(within(dialog).getByText(/链接和表单字段保持可交互/)).toBeInTheDocument();
    expect(within(dialog).getByText(/原文档不会被修改/)).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: "导出…" }));

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("export_flattened_copy", {
        documentId: "doc-1",
        destination: "/tmp/report-flattened.pdf",
        confirmUnencryptedOutput: false,
      }),
    );
    expect(saveMock).toHaveBeenCalledWith({
      defaultPath: "report-flattened.pdf",
      filters: [{ name: "PDF", extensions: ["pdf"] }],
    });
    const notice = await screen.findByRole("status");
    expect(notice).toHaveTextContent("已导出 Flatten 副本");
    expect(notice).toHaveTextContent("flatten 4 个标注");
    expect(notice).toHaveTextContent("保留 2 个交互项");
    expect(notice).toHaveTextContent("跳过 2 个");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("does not export when the save dialog is cancelled", async () => {
    saveMock.mockResolvedValue(null);
    const { user, dialog } = await openFlattenDialog();

    await user.click(within(dialog).getByRole("button", { name: "导出…" }));

    await waitFor(() => expect(saveMock).toHaveBeenCalled());
    expect(invokeMock).not.toHaveBeenCalled();
    expect(
      screen.getByRole("dialog", { name: "导出 Flatten 副本" }),
    ).toBeInTheDocument();
  });

  it("explains why the source PDF cannot be the flattened-copy target", async () => {
    saveMock.mockResolvedValue("/tmp/report.pdf");
    invokeMock.mockRejectedValue({
      code: "export_target_is_source",
      message: "/tmp/report.pdf is the open Source Document",
    });
    const { user, dialog } = await openFlattenDialog();

    await user.click(within(dialog).getByRole("button", { name: "导出…" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "不能覆盖源 PDF 文件，请选择其他位置。",
    );
    expect(
      screen.getByRole("dialog", { name: "导出 Flatten 副本" }),
    ).toBeInTheDocument();
  });

  it("explains why the current output PDF cannot be the flattened-copy target", async () => {
    saveMock.mockResolvedValue("/tmp/report-output.pdf");
    invokeMock.mockRejectedValue({
      code: "export_target_is_output",
      message: "/tmp/report-output.pdf is the current Output Document",
    });
    const { user, dialog } = await openFlattenDialog();

    await user.click(within(dialog).getByRole("button", { name: "导出…" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "不能覆盖当前输出 PDF 文件，请选择其他位置。",
    );
  });

  it("warns that an encrypted source produces an unencrypted copy", async () => {
    saveMock.mockResolvedValue("/tmp/report-flattened.pdf");
    invokeMock.mockResolvedValue(flattenResult());
    const user = userEvent.setup();
    renderPrintExport(documentView({ encrypted: true }));

    await user.click(
      screen.getByRole("button", { name: "导出 Flatten 副本…" }),
    );
    const dialog = screen.getByRole("dialog", { name: "导出 Flatten 副本" });
    expect(within(dialog).getByText(/将以未加密方式保存/)).toBeInTheDocument();

    const submit = within(dialog).getByRole("button", { name: "导出…" });
    expect(submit).toBeDisabled();

    await user.click(
      within(dialog).getByRole("checkbox", {
        name: "我了解副本将不加密，继续导出",
      }),
    );
    expect(submit).toBeEnabled();
    await user.click(submit);

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("export_flattened_copy", {
        documentId: "doc-1",
        destination: "/tmp/report-flattened.pdf",
        confirmUnencryptedOutput: true,
      }),
    );
  });

  it("shows a localized retryable error and keeps the dialog open when flattening fails", async () => {
    saveMock.mockResolvedValue("/tmp/report-flattened.pdf");
    invokeMock.mockRejectedValue({
      code: "flatten_export_failed",
      message: "could not draw annotation",
    });
    const { user, dialog } = await openFlattenDialog();

    await user.click(within(dialog).getByRole("button", { name: "导出…" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "无法生成 Flatten 副本，请检查文档后重试。",
    );
    expect(
      screen.getByRole("dialog", { name: "导出 Flatten 副本" }),
    ).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "导出…" })).toBeEnabled();
  });
});

function pageInfoFor(pageIndex: number) {
  return {
    pageIndex,
    width: 612,
    height: 792,
    rotation: 0,
    cropX: 0,
    cropY: 0,
  };
}

function printBackend() {
  return (command: string, args: unknown) => {
    switch (command) {
      case "page_info":
        return Promise.resolve(
          pageInfoFor((args as { pageIndex: number }).pageIndex),
        );
      case "render_page": {
        const { pageIndex, scale } = args as { pageIndex: number; scale: number };
        return Promise.resolve({
          documentId: "doc-1",
          pageIndex,
          scale,
          width: Math.round(612 * scale),
          height: Math.round(792 * scale),
          pngBase64: "iVBORw0KGgo=",
        });
      }
      case "print_document":
        return Promise.resolve(undefined);
      default:
        return Promise.reject(new Error(`unexpected command ${command}`));
    }
  };
}

function printCalls() {
  return invokeMock.mock.calls.filter(([command]) => command === "render_page");
}

describe("PrintExport printing", () => {
  it("prepares one print page per selected page and then opens the print dialog", async () => {
    const user = userEvent.setup();
    invokeMock.mockImplementation(printBackend());
    renderPrintExport();

    await user.click(screen.getByRole("button", { name: "打印…" }));
    const dialog = screen.getByRole("dialog", { name: "打印" });
    expect(within(dialog).getByLabelText("全部页面（3 页）")).toBeChecked();
    expect(within(dialog).getByLabelText("当前页（第 2 页）")).not.toBeChecked();

    await user.click(
      within(dialog).getByRole("button", { name: "打印…" }),
    );

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("print_document", {
        documentId: "doc-1",
      }),
    );
    expect(
      printCalls().map(([, args]) => (args as { pageIndex: number }).pageIndex),
    ).toEqual([0, 1, 2]);
    for (const [, args] of printCalls()) {
      expect((args as { scale: number }).scale).toBeCloseTo(150 / 72, 5);
    }

    const printRoot = document.querySelector(".print-root");
    expect(printRoot).not.toBeNull();
    expect(printRoot!.querySelectorAll("img")).toHaveLength(3);
    expect(
      Array.from(printRoot!.querySelectorAll("img")).map((image) =>
        image.getAttribute("src"),
      ),
    ).toEqual(["blob:pdfx-print-1", "blob:pdfx-print-2", "blob:pdfx-print-3"]);
    expect(printRoot!.innerHTML).not.toContain("base64");
    expect(printRoot!.querySelector("style")!.textContent).toContain(
      "size: 612pt 792pt",
    );
    const commands = invokeMock.mock.calls.map(([command]) => command);
    // Printing only ever reads pages and opens the dialog; it never touches
    // edit history, so it cannot create an unsaved marker.
    expect(new Set(commands)).toEqual(
      new Set(["page_info", "render_page", "print_document"]),
    );
    expect(commands[commands.length - 1]).toBe("print_document");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("已准备打印：3 页");

    window.dispatchEvent(new Event("afterprint"));
    await waitFor(() =>
      expect(document.querySelector(".print-root")).toBeNull(),
    );
    expect(revokedObjectUrls).toEqual(createdObjectUrls);
    expect(revokedObjectUrls).toHaveLength(3);
  });

  it("honours a custom page range, orientation and actual-size scaling", async () => {
    const user = userEvent.setup();
    invokeMock.mockImplementation(printBackend());
    renderPrintExport();

    await user.click(screen.getByRole("button", { name: "打印…" }));
    const dialog = screen.getByRole("dialog", { name: "打印" });
    await user.click(within(dialog).getByLabelText("自定义"));
    const from = within(dialog).getByLabelText("起始页");
    const to = within(dialog).getByLabelText("结束页");
    await user.clear(from);
    await user.type(from, "2");
    await user.clear(to);
    await user.type(to, "3");
    await user.selectOptions(within(dialog).getByLabelText("方向"), "landscape");
    await user.selectOptions(within(dialog).getByLabelText("缩放"), "actual");

    await user.click(
      within(dialog).getByRole("button", { name: "打印…" }),
    );

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("print_document", {
        documentId: "doc-1",
      }),
    );
    expect(
      printCalls().map(([, args]) => (args as { pageIndex: number }).pageIndex),
    ).toEqual([1, 2]);
    const printRoot = document.querySelector(".print-root")!;
    expect(printRoot.querySelector("style")!.textContent).toContain(
      "size: 792pt 612pt",
    );
    // Every printed sheet uses the job's chosen orientation, and an actual-size
    // image keeps the Page's true dimensions in points.
    for (const page of printRoot.querySelectorAll(".print-page")) {
      expect(page).toHaveStyle({ width: "792pt", height: "612pt" });
    }
    for (const image of printRoot.querySelectorAll("img")) {
      expect(image).toHaveClass("print-page-image--actual");
      expect(image).toHaveStyle({ width: "612pt", height: "792pt" });
    }
  });

  it("rejects an invalid custom range without rendering or printing", async () => {
    const user = userEvent.setup();
    invokeMock.mockImplementation(printBackend());
    renderPrintExport();

    await user.click(screen.getByRole("button", { name: "打印…" }));
    const dialog = screen.getByRole("dialog", { name: "打印" });
    await user.click(within(dialog).getByLabelText("自定义"));
    const from = within(dialog).getByLabelText("起始页");
    await user.clear(from);
    await user.type(from, "3");
    const to = within(dialog).getByLabelText("结束页");
    await user.clear(to);
    await user.type(to, "1");

    await user.click(
      within(dialog).getByRole("button", { name: "打印…" }),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "请输入有效的页面范围。",
    );
    expect(printCalls()).toHaveLength(0);
    expect(invokeMock).not.toHaveBeenCalledWith("print_document", {
      documentId: "doc-1",
    });
  });

  it("shows preparation progress for a large document and can cancel it", async () => {
    const user = userEvent.setup();
    let resolveRender: ((view: unknown) => void) | undefined;
    invokeMock.mockImplementation((command, args) => {
      if (command === "page_info") {
        return Promise.resolve(
          pageInfoFor((args as { pageIndex: number }).pageIndex),
        );
      }
      if (command === "render_page") {
        return new Promise((resolve) => {
          resolveRender = resolve;
        });
      }
      return Promise.resolve(undefined);
    });
    renderPrintExport(documentView({ pageCount: 200 }));

    await user.click(screen.getByRole("button", { name: "打印…" }));
    const dialog = screen.getByRole("dialog", { name: "打印" });
    await user.click(
      within(dialog).getByRole("button", { name: "打印…" }),
    );

    expect(
      await within(dialog).findByText("正在准备第 1 / 200 页…"),
    ).toBeInTheDocument();
    expect(within(dialog).getByRole("progressbar")).toHaveValue(0);

    await user.click(within(dialog).getByRole("button", { name: "取消打印" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    resolveRender?.({
      documentId: "doc-1",
      pageIndex: 0,
      scale: 150 / 72,
      width: 1275,
      height: 1650,
      pngBase64: "iVBORw0KGgo=",
    });

    await waitFor(() => expect(printCalls()).toHaveLength(1));
    expect(invokeMock).not.toHaveBeenCalledWith("print_document", {
      documentId: "doc-1",
    });
    expect(document.querySelector(".print-root")).toBeNull();
    expect(createdObjectUrls).toHaveLength(0);
    expect(revokedObjectUrls).toHaveLength(0);
  });

  it("releases prepared print bitmaps when preparation is cancelled", async () => {
    const user = userEvent.setup();
    let renderCount = 0;
    let resolvePendingRender: ((view: unknown) => void) | undefined;
    invokeMock.mockImplementation((command, args) => {
      if (command === "page_info") {
        return Promise.resolve(
          pageInfoFor((args as { pageIndex: number }).pageIndex),
        );
      }
      if (command === "render_page") {
        renderCount += 1;
        if (renderCount <= 2) {
          return Promise.resolve({
            documentId: "doc-1",
            pageIndex: renderCount - 1,
            scale: 150 / 72,
            width: 1275,
            height: 1650,
            pngBase64: "iVBORw0KGgo=",
          });
        }
        return new Promise((resolve) => {
          resolvePendingRender = resolve;
        });
      }
      return Promise.resolve(undefined);
    });
    renderPrintExport(documentView({ pageCount: 100 }));

    await user.click(screen.getByRole("button", { name: "打印…" }));
    const dialog = screen.getByRole("dialog", { name: "打印" });
    await user.click(
      within(dialog).getByRole("button", { name: "打印…" }),
    );

    expect(
      await within(dialog).findByText("正在准备第 3 / 100 页…"),
    ).toBeInTheDocument();
    expect(createdObjectUrls).toHaveLength(2);
    expect(revokedObjectUrls).toHaveLength(0);

    await user.click(within(dialog).getByRole("button", { name: "取消打印" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(new Set(revokedObjectUrls)).toEqual(new Set(createdObjectUrls));

    resolvePendingRender?.({
      documentId: "doc-1",
      pageIndex: 2,
      scale: 150 / 72,
      width: 1275,
      height: 1650,
      pngBase64: "iVBORw0KGgo=",
    });

    await waitFor(() => expect(printCalls()).toHaveLength(3));
    expect(createdObjectUrls).toHaveLength(2);
    expect(revokedObjectUrls).toHaveLength(2);
    expect(invokeMock).not.toHaveBeenCalledWith("print_document", {
      documentId: "doc-1",
    });
    expect(document.querySelector(".print-root")).toBeNull();
  });

  it("releases prepared print bitmaps when the component unmounts mid-preparation", async () => {
    const user = userEvent.setup();
    let renderCount = 0;
    let resolvePendingRender: ((view: unknown) => void) | undefined;
    invokeMock.mockImplementation((command, args) => {
      if (command === "page_info") {
        return Promise.resolve(
          pageInfoFor((args as { pageIndex: number }).pageIndex),
        );
      }
      if (command === "render_page") {
        renderCount += 1;
        if (renderCount <= 2) {
          return Promise.resolve({
            documentId: "doc-1",
            pageIndex: renderCount - 1,
            scale: 150 / 72,
            width: 1275,
            height: 1650,
            pngBase64: "iVBORw0KGgo=",
          });
        }
        return new Promise((resolve) => {
          resolvePendingRender = resolve;
        });
      }
      return Promise.resolve(undefined);
    });
    const ref = createRef<PrintExportHandle>();
    const { unmount } = render(
      <PrintExport
        ref={ref}
        document={documentView({ pageCount: 100 })}
        pageIndex={1}
      />,
    );

    await user.click(screen.getByRole("button", { name: "打印…" }));
    const dialog = screen.getByRole("dialog", { name: "打印" });
    await user.click(
      within(dialog).getByRole("button", { name: "打印…" }),
    );
    expect(
      await within(dialog).findByText("正在准备第 3 / 100 页…"),
    ).toBeInTheDocument();
    expect(createdObjectUrls).toHaveLength(2);

    unmount();

    expect(new Set(revokedObjectUrls)).toEqual(new Set(createdObjectUrls));
    resolvePendingRender?.({
      documentId: "doc-1",
      pageIndex: 2,
      scale: 150 / 72,
      width: 1275,
      height: 1650,
      pngBase64: "iVBORw0KGgo=",
    });
    await waitFor(() => expect(printCalls()).toHaveLength(3));
    expect(createdObjectUrls).toHaveLength(2);
    expect(invokeMock).not.toHaveBeenCalledWith("print_document", {
      documentId: "doc-1",
    });
  });

  it("prepares and decodes a full large print range one page at a time", async () => {
    const user = userEvent.setup();
    const decoding = {
      live: 0,
      maxLive: 0,
      total: 0,
      rendersAtFirstDecode: -1,
    };

    class TrackedPrintImage {
      onload: (() => void) | null = null;
      onerror: (() => void) | null = null;
      src = "";

      async decode(): Promise<void> {
        if (decoding.rendersAtFirstDecode < 0) {
          decoding.rendersAtFirstDecode = printCalls().length;
        }
        decoding.live += 1;
        decoding.total += 1;
        decoding.maxLive = Math.max(decoding.maxLive, decoding.live);
        await Promise.resolve();
        decoding.live -= 1;
      }
    }

    vi.stubGlobal("Image", TrackedPrintImage);
    invokeMock.mockImplementation(printBackend());
    renderPrintExport(documentView({ pageCount: 2000 }));

    await user.click(screen.getByRole("button", { name: "打印…" }));
    const dialog = screen.getByRole("dialog", { name: "打印" });
    await user.click(
      within(dialog).getByRole("button", { name: "打印…" }),
    );

    await waitFor(
      () =>
        expect(invokeMock).toHaveBeenCalledWith("print_document", {
          documentId: "doc-1",
        }),
      { timeout: 30000 },
    );

    expect(printCalls()).toHaveLength(2000);
    // The hard cap: only one page's bitmap is decoded at a time, no matter how
    // many pages the job contains.
    expect(decoding.maxLive).toBe(1);
    expect(decoding.total).toBe(2000);
    // Decoding starts while pages are still being rendered, not after all of
    // them have accumulated.
    expect(decoding.rendersAtFirstDecode).toBeLessThan(2000);

    const printRoot = document.querySelector(".print-root")!;
    const images = Array.from(printRoot.querySelectorAll("img"));
    expect(images).toHaveLength(2000);
    for (const image of images) {
      expect(image.getAttribute("src")).toMatch(/^blob:pdfx-print-/);
    }
    expect(printRoot.innerHTML).not.toContain("base64");
    expect(createdObjectUrls).toHaveLength(2000);
  }, 60000);
});
