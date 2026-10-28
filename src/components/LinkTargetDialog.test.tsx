import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { LinkTargetDialog } from "./LinkTargetDialog";

describe("LinkTargetDialog", () => {
  it("submits a trimmed URL target in create mode", () => {
    const onSubmit = vi.fn();
    render(
      <LinkTargetDialog
        mode="create"
        pageCount={10}
        initialTarget={null}
        busy={false}
        error={null}
        onCancel={vi.fn()}
        onSubmit={onSubmit}
      />,
    );

    expect(
      screen.getByRole("dialog", { name: "添加链接" }),
    ).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("链接网址"), {
      target: { value: "  https://example.com/docs  " },
    });
    fireEvent.click(screen.getByRole("button", { name: "添加" }));

    expect(onSubmit).toHaveBeenCalledWith({
      kind: "url",
      url: "https://example.com/docs",
    });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("validates the page number against the document's page count", () => {
    const onSubmit = vi.fn();
    render(
      <LinkTargetDialog
        mode="create"
        pageCount={10}
        initialTarget={null}
        busy={false}
        error={null}
        onCancel={vi.fn()}
        onSubmit={onSubmit}
      />,
    );

    fireEvent.click(screen.getByRole("radio", { name: "页面" }));
    expect(screen.getByText("第 1 至 10 页")).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("目标页码"), {
      target: { value: "11" },
    });
    fireEvent.click(screen.getByRole("button", { name: "添加" }));

    expect(screen.getByRole("alert")).toHaveTextContent(
      "页码必须在 1 到 10 之间。",
    );
    expect(onSubmit).not.toHaveBeenCalled();

    fireEvent.change(screen.getByLabelText("目标页码"), {
      target: { value: "3" },
    });
    fireEvent.click(screen.getByRole("button", { name: "添加" }));

    expect(onSubmit).toHaveBeenCalledWith({ kind: "page", pageIndex: 2 });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows the Chinese reason for an invalid URL and does not submit", () => {
    const onSubmit = vi.fn();
    render(
      <LinkTargetDialog
        mode="create"
        pageCount={10}
        initialTarget={null}
        busy={false}
        error={null}
        onCancel={vi.fn()}
        onSubmit={onSubmit}
      />,
    );

    fireEvent.change(screen.getByLabelText("链接网址"), {
      target: { value: "ftp://example.com" },
    });
    fireEvent.click(screen.getByRole("button", { name: "添加" }));

    expect(screen.getByRole("alert")).toHaveTextContent(
      "链接网址必须以 http://、https:// 或 mailto: 开头。",
    );
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("prefills the current URL target in edit mode", () => {
    const onSubmit = vi.fn();
    render(
      <LinkTargetDialog
        mode="edit"
        pageCount={10}
        initialTarget={{ kind: "url", url: "https://example.com/old" }}
        busy={false}
        error={null}
        onCancel={vi.fn()}
        onSubmit={onSubmit}
      />,
    );

    expect(screen.getByRole("dialog", { name: "编辑链接" })).toBeInTheDocument();
    expect(screen.getByLabelText("链接网址")).toHaveValue(
      "https://example.com/old",
    );
    fireEvent.change(screen.getByLabelText("链接网址"), {
      target: { value: "https://example.com/new" },
    });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    expect(onSubmit).toHaveBeenCalledWith({
      kind: "url",
      url: "https://example.com/new",
    });
  });

  it("prefills the current page target and its one-based number", () => {
    render(
      <LinkTargetDialog
        mode="edit"
        pageCount={10}
        initialTarget={{ kind: "page", pageIndex: 4 }}
        busy={false}
        error={null}
        onCancel={vi.fn()}
        onSubmit={vi.fn()}
      />,
    );

    expect(screen.getByRole("radio", { name: "页面" })).toBeChecked();
    expect(screen.getByLabelText("目标页码")).toHaveValue("5");
  });

  it("shows a backend error and disables both actions while busy", () => {
    const onCancel = vi.fn();
    render(
      <LinkTargetDialog
        mode="edit"
        pageCount={10}
        initialTarget={{ kind: "url", url: "https://example.com" }}
        busy
        error="此链接无法编辑，已保持原样。"
        onCancel={onCancel}
        onSubmit={vi.fn()}
      />,
    );

    expect(screen.getByRole("alert")).toHaveTextContent(
      "此链接无法编辑，已保持原样。",
    );
    expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "取消" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("cancels without submitting", () => {
    const onCancel = vi.fn();
    const onSubmit = vi.fn();
    render(
      <LinkTargetDialog
        mode="create"
        pageCount={10}
        initialTarget={null}
        busy={false}
        error={null}
        onCancel={onCancel}
        onSubmit={onSubmit}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onSubmit).not.toHaveBeenCalled();
  });
});
