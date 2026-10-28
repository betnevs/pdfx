import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  EditHistoryView,
  LinkMutationView,
  LinkView,
} from "../lib/backend";
import { deleteLink, updateLinkTarget } from "../lib/backend";
import { DOCUMENT_CHANGED_EVENT } from "../lib/documentEvents";
import { LinkLayer } from "./LinkLayer";

vi.mock("../lib/backend", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/backend")>();
  return {
    ...actual,
    updateLinkTarget: vi.fn(),
    deleteLink: vi.fn(),
  };
});

const updateLinkTargetMock = vi.mocked(updateLinkTarget);
const deleteLinkMock = vi.mocked(deleteLink);

const dirtyHistory: EditHistoryView = {
  canUndo: true,
  canRedo: false,
  revision: 2,
  savedRevision: 0,
  dirty: true,
};

const urlLink: LinkView = {
  id: "0:0",
  name: "pdfx-1",
  pageIndex: 0,
  target: { kind: "url", url: "https://example.com/one" },
  unsupported: null,
  rects: [{ x: 72, y: 100, width: 108, height: 20 }],
};

const multiLinePageLink: LinkView = {
  id: "0:1",
  name: "pdfx-2",
  pageIndex: 0,
  target: { kind: "page", pageIndex: 2 },
  unsupported: null,
  rects: [
    { x: 72, y: 100, width: 200, height: 20 },
    { x: 72, y: 122, width: 150, height: 20 },
  ],
};

const scriptLink: LinkView = {
  id: "0:2",
  name: null,
  pageIndex: 0,
  target: null,
  unsupported: "action",
  rects: [{ x: 300, y: 400, width: 60, height: 18 }],
};

const links = [urlLink, multiLinePageLink, scriptLink];

function renderLayer(
  overrides: Partial<Parameters<typeof LinkLayer>[0]> = {},
) {
  const onLinksChanged = vi.fn();
  render(
    <LinkLayer
      documentId="doc-1"
      pageIndex={0}
      pageWidth={612}
      pageHeight={792}
      pageCount={10}
      readOnly={false}
      canAnnotate
      links={links}
      onLinksChanged={onLinksChanged}
      {...overrides}
    />,
  );
  return { onLinksChanged };
}

function selectLink(linkId: string) {
  fireEvent.click(screen.getByRole("button", { name: "链接" }));
  fireEvent.click(screen.getByTestId(`link-region-${linkId}-0`));
}

beforeEach(() => {
  updateLinkTargetMock.mockReset();
  deleteLinkMock.mockReset();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("LinkLayer", () => {
  it("explains that links need annotation permission and renders no regions", () => {
    renderLayer({ canAnnotate: false });

    const toggle = screen.getByRole("button", { name: "链接" });
    expect(toggle).toBeDisabled();
    expect(toggle).toHaveAttribute("title", "此文档不允许添加注释");
    expect(screen.getByText("此文档不允许添加注释")).toBeInTheDocument();
    expect(screen.queryByTestId("link-region-0:0-0")).not.toBeInTheDocument();
  });

  it("explains that a read-only document cannot change links", () => {
    renderLayer({ readOnly: true });

    const toggle = screen.getByRole("button", { name: "链接" });
    expect(toggle).toBeDisabled();
    expect(toggle).toHaveAttribute("title", "此文档不允许编辑内容");
    expect(screen.getByText("此文档不允许编辑内容")).toBeInTheDocument();
  });

  it("shows the link regions only while the tool is armed", () => {
    renderLayer();

    expect(screen.queryByTestId("link-region-0:0-0")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "链接" }));
    expect(screen.getByTestId("link-region-0:0-0")).toBeInTheDocument();
    expect(screen.getByTestId("link-region-0:1-1")).toBeInTheDocument();
  });

  it("outlines the selected link region and names its URL target", () => {
    renderLayer();
    selectLink(urlLink.id);

    const selection = screen.getByTestId("link-selection");
    expect(selection).toHaveStyle({
      left: `${(72 / 612) * 100}%`,
      top: `${(100 / 792) * 100}%`,
      width: `${(108 / 612) * 100}%`,
      height: `${(20 / 792) * 100}%`,
    });
    expect(screen.getByTestId("link-target")).toHaveTextContent(
      "https://example.com/one",
    );
    expect(screen.getByRole("button", { name: "编辑链接" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "删除链接" })).toBeEnabled();
  });

  it("draws one selection box per quad and labels a page target", () => {
    renderLayer();
    fireEvent.click(screen.getByRole("button", { name: "链接" }));
    fireEvent.click(screen.getByTestId("link-region-0:1-1"));

    expect(screen.getAllByTestId("link-selection")).toHaveLength(2);
    expect(screen.getByTestId("link-target")).toHaveTextContent("第 3 页");
  });

  it("edits the selected link's target through the Chinese dialog", async () => {
    updateLinkTargetMock.mockResolvedValue({
      link: { ...urlLink, target: { kind: "url", url: "https://example.com/two" } },
      pageIndex: 0,
      history: dirtyHistory,
    });
    const changed = vi.fn();
    window.addEventListener(DOCUMENT_CHANGED_EVENT, changed);
    const { onLinksChanged } = renderLayer();
    selectLink(urlLink.id);

    fireEvent.click(screen.getByRole("button", { name: "编辑链接" }));
    expect(screen.getByRole("dialog", { name: "编辑链接" })).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("链接网址"), {
      target: { value: "https://example.com/two" },
    });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() => {
      expect(updateLinkTargetMock).toHaveBeenCalledWith("doc-1", {
        linkId: "0:0",
        target: { kind: "url", url: "https://example.com/two" },
      });
    });
    expect(onLinksChanged).toHaveBeenCalledTimes(1);
    expect(changed).toHaveBeenCalled();
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(screen.getByRole("status")).toHaveTextContent("链接已更新");
    window.removeEventListener(DOCUMENT_CHANGED_EVENT, changed);
  });

  it("deletes the selected link and clears the selection", async () => {
    deleteLinkMock.mockResolvedValue({
      link: null,
      pageIndex: 0,
      history: dirtyHistory,
    } satisfies LinkMutationView);
    const changed = vi.fn();
    window.addEventListener(DOCUMENT_CHANGED_EVENT, changed);
    const { onLinksChanged } = renderLayer();
    selectLink(urlLink.id);

    fireEvent.click(screen.getByRole("button", { name: "删除链接" }));

    await waitFor(() => {
      expect(deleteLinkMock).toHaveBeenCalledWith("doc-1", "0:0");
    });
    expect(onLinksChanged).toHaveBeenCalledTimes(1);
    expect(changed).toHaveBeenCalled();
    await waitFor(() => {
      expect(screen.queryByTestId("link-selection")).not.toBeInTheDocument();
    });
    expect(screen.getByRole("status")).toHaveTextContent("链接已删除");
    window.removeEventListener(DOCUMENT_CHANGED_EVENT, changed);
  });

  it("explains an unsupported link and disables edit and delete", () => {
    renderLayer();
    fireEvent.click(screen.getByRole("button", { name: "链接" }));
    fireEvent.click(screen.getByTestId("link-region-0:2-0"));

    expect(
      screen.getByText("此链接使用 JavaScript 等 PDFX 不支持的操作，无法编辑。"),
    ).toBeInTheDocument();
    expect(screen.getByTestId("link-selection")).toBeInTheDocument();
    const edit = screen.getByRole("button", { name: "编辑链接" });
    const remove = screen.getByRole("button", { name: "删除链接" });
    expect(edit).toBeDisabled();
    expect(remove).toBeDisabled();
    expect(edit).toHaveAttribute(
      "title",
      "此链接使用 JavaScript 等 PDFX 不支持的操作，无法编辑。",
    );
  });

  it("keeps the dialog and selection visible when the backend refuses the edit", async () => {
    updateLinkTargetMock.mockRejectedValue({
      code: "unsupported_link",
      message: "unsupported",
    });
    const { onLinksChanged } = renderLayer();
    selectLink(urlLink.id);

    fireEvent.click(screen.getByRole("button", { name: "编辑链接" }));
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "此链接无法编辑，已保持原样。",
    );
    expect(screen.getByRole("dialog", { name: "编辑链接" })).toBeInTheDocument();
    expect(screen.getByTestId("link-selection")).toBeInTheDocument();
    expect(onLinksChanged).not.toHaveBeenCalled();
  });
});
