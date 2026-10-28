import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { PrototypeApp } from "./PrototypeApp";
describe("M0 prototype workspace", () => {
  it("renders the full PDFX editing workspace shell", () => {
    render(<PrototypeApp />);

    expect(screen.getByRole("heading", { level: 1, name: "PDFX" })).toBeInTheDocument();
    expect(screen.getByText("2026 产品路线图.pdf")).toBeInTheDocument();

    const modeSwitch = screen.getByRole("group", { name: "工作模式" });
    expect(within(modeSwitch).getByRole("button", { name: "注释" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(within(modeSwitch).getByRole("button", { name: "编辑" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );

    const tabs = screen.getByRole("tablist", { name: "编辑工具页签" });
    for (const name of ["注释", "形状", "插入", "编辑文本", "表单"]) {
      expect(within(tabs).getByRole("tab", { name })).toBeInTheDocument();
    }

    const thumbnails = screen.getByRole("navigation", { name: "页面缩略图" });
    expect(within(thumbnails).getByRole("button", { name: "第 1 页" })).toBeInTheDocument();
    expect(within(thumbnails).getByRole("button", { name: "第 8 页" })).toBeInTheDocument();

    const canvas = screen.getByRole("main", { name: "页面画布" });
    expect(within(canvas).getByText("2026 产品路线图")).toBeInTheDocument();

    expect(screen.getByRole("complementary", { name: "属性面板" })).toBeInTheDocument();

    const statusBar = screen.getByRole("contentinfo");
    expect(within(statusBar).getByRole("spinbutton", { name: "当前页码" })).toHaveValue(1);
    expect(within(statusBar).getByText("共 8 页")).toBeInTheDocument();
    expect(within(statusBar).getByRole("textbox", { name: "缩放比例" })).toHaveValue("100%");
    expect(within(statusBar).getByRole("button", { name: "适合宽度" })).toBeInTheDocument();
    expect(within(statusBar).getByRole("button", { name: "适合页面" })).toBeInTheDocument();
  });

  it("keeps mode, tab, toolbar and canvas tool state in sync", async () => {
    const user = userEvent.setup();
    render(<PrototypeApp />);

    const tabs = screen.getByRole("tablist", { name: "编辑工具页签" });
    const modeSwitch = screen.getByRole("group", { name: "工作模式" });

    expect(within(modeSwitch).getByRole("button", { name: "注释" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("heading", { name: "注释工具" })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("当前工具：高亮");

    await user.click(within(tabs).getByRole("tab", { name: "形状" }));
    expect(within(tabs).getByRole("tab", { name: "形状" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(within(modeSwitch).getByRole("button", { name: "注释" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("heading", { name: "形状工具" })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("当前工具：矩形");
    const shapeTools = screen.getByRole("group", { name: "形状工具" });
    expect(within(shapeTools).getByRole("button", { name: "矩形" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );

    await user.click(within(shapeTools).getByRole("button", { name: "箭头" }));
    expect(within(shapeTools).getByRole("button", { name: "箭头" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("status")).toHaveTextContent("当前工具：箭头");

    await user.click(within(tabs).getByRole("tab", { name: "编辑文本" }));
    expect(within(tabs).getByRole("tab", { name: "编辑文本" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(within(modeSwitch).getByRole("button", { name: "编辑" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("heading", { name: "编辑文本" })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("当前工具：编辑文字");

    await user.click(within(modeSwitch).getByRole("button", { name: "注释" }));
    expect(within(tabs).getByRole("tab", { name: "注释" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(within(modeSwitch).getByRole("button", { name: "注释" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("status")).toHaveTextContent("当前工具：高亮");
  });

  it("navigates pages through thumbnails, the page input and prev/next", async () => {
    const user = userEvent.setup();
    render(<PrototypeApp />);

    const thumbnails = screen.getByRole("navigation", { name: "页面缩略图" });
    const statusBar = screen.getByRole("contentinfo");
    const canvas = screen.getByRole("main", { name: "页面画布" });
    const pageInput = within(statusBar).getByRole("spinbutton", { name: "当前页码" });

    expect(pageInput).toHaveValue(1);
    expect(within(thumbnails).getByRole("button", { name: "第 1 页" })).toHaveAttribute(
      "aria-current",
      "page",
    );

    await user.click(within(thumbnails).getByRole("button", { name: "第 3 页" }));
    expect(pageInput).toHaveValue(3);
    expect(within(thumbnails).getByRole("button", { name: "第 3 页" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(within(thumbnails).getByRole("button", { name: "第 1 页" })).not.toHaveAttribute(
      "aria-current",
    );
    expect(
      within(canvas).getByRole("heading", { name: "文件从不离开这台设备" }),
    ).toBeInTheDocument();

    await user.click(within(statusBar).getByRole("button", { name: "下一页" }));
    expect(pageInput).toHaveValue(4);
    expect(
      within(canvas).getByRole("heading", { name: "这次原型要回答什么" }),
    ).toBeInTheDocument();

    await user.click(within(statusBar).getByRole("button", { name: "上一页" }));
    expect(pageInput).toHaveValue(3);

    await user.clear(pageInput);
    await user.type(pageInput, "6{Enter}");
    expect(pageInput).toHaveValue(6);
    expect(
      within(canvas).getByRole("heading", {
        name: "注释要能被别的阅读器继续编辑",
      }),
    ).toBeInTheDocument();

    await user.clear(pageInput);
    await user.type(pageInput, "99{Enter}");
    expect(pageInput).toHaveValue(8);
    expect(within(statusBar).getByRole("button", { name: "下一页" })).toBeDisabled();

    await user.clear(pageInput);
    await user.type(pageInput, "0{Enter}");
    expect(pageInput).toHaveValue(1);
    expect(within(statusBar).getByRole("button", { name: "上一页" })).toBeDisabled();
  });

  it("changes zoom through the percentage input, steps and fit modes", async () => {
    const user = userEvent.setup();
    render(<PrototypeApp />);

    const statusBar = screen.getByRole("contentinfo");
    const canvas = screen.getByRole("main", { name: "页面画布" });
    const zoomInput = within(statusBar).getByRole("textbox", { name: "缩放比例" });
    const zoomIn = within(statusBar).getByRole("button", { name: "放大" });
    const zoomOut = within(statusBar).getByRole("button", { name: "缩小" });
    const fitWidth = within(statusBar).getByRole("button", { name: "适合宽度" });
    const fitPage = within(statusBar).getByRole("button", { name: "适合页面" });

    expect(zoomInput).toHaveValue("100%");
    expect(canvas.querySelector(".page-frame")).toHaveStyle({ width: "720px" });

    await user.click(zoomIn);
    expect(zoomInput).toHaveValue("125%");
    await user.click(zoomOut);
    expect(zoomInput).toHaveValue("100%");

    await user.clear(zoomInput);
    await user.type(zoomInput, "250{Enter}");
    expect(zoomInput).toHaveValue("250%");
    expect(canvas.querySelector(".page-frame")).toHaveStyle({ width: "1800px" });

    await user.clear(zoomInput);
    await user.type(zoomInput, "9999{Enter}");
    expect(zoomInput).toHaveValue("4000%");

    await user.clear(zoomInput);
    await user.type(zoomInput, "1{Enter}");
    expect(zoomInput).toHaveValue("10%");

    await user.clear(zoomInput);
    await user.type(zoomInput, "abc{Enter}");
    expect(zoomInput).toHaveValue("100%");

    await user.click(fitWidth);
    expect(fitWidth).toHaveAttribute("aria-pressed", "true");
    expect(fitPage).toHaveAttribute("aria-pressed", "false");
    const widthZoom = Number.parseInt((zoomInput as HTMLInputElement).value, 10);
    expect(widthZoom).toBeGreaterThanOrEqual(10);
    expect(widthZoom).toBeLessThanOrEqual(4000);

    await user.click(fitPage);
    expect(fitPage).toHaveAttribute("aria-pressed", "true");
    expect(fitWidth).toHaveAttribute("aria-pressed", "false");
    const pageZoom = Number.parseInt((zoomInput as HTMLInputElement).value, 10);
    expect(pageZoom).toBeGreaterThanOrEqual(10);
    expect(pageZoom).toBeLessThanOrEqual(widthZoom);

    await user.click(zoomIn);
    expect(fitPage).toHaveAttribute("aria-pressed", "false");
    expect(fitWidth).toHaveAttribute("aria-pressed", "false");
    expect(zoomInput).toHaveValue(`${pageZoom + 25}%`);
  });

  it("shows matching properties for selected text, annotations and shapes", async () => {
    const user = userEvent.setup();
    render(<PrototypeApp />);

    const properties = screen.getByRole("complementary", { name: "属性面板" });
    const lead = screen.getByRole("button", { name: /^文本：把 PDF 编辑/ });

    await user.click(lead);
    expect(within(properties).getByRole("heading", { name: "文本属性" })).toBeInTheDocument();
    expect(within(properties).getByText(/已选中文本：把 PDF 编辑/)).toBeInTheDocument();
    expect(within(properties).getByRole("combobox", { name: "字体" })).toHaveValue("serif");

    const fontSize = within(properties).getByRole("spinbutton", { name: "字号" });
    expect(fontSize).toHaveValue(17);
    await user.clear(fontSize);
    await user.type(fontSize, "24");
    expect(fontSize).toHaveValue(24);
    expect(lead).toHaveStyle({ fontSize: "24px" });

    const red = within(properties).getByRole("button", { name: "文字颜色：红色" });
    await user.click(red);
    expect(red).toHaveAttribute("aria-pressed", "true");
    expect(lead).toHaveStyle({ color: "#d93b30" });

    const bold = within(properties).getByRole("button", { name: "粗体" });
    await user.click(bold);
    expect(bold).toHaveAttribute("aria-pressed", "true");
    expect(lead).toHaveStyle({ fontWeight: "700" });

    const center = within(properties).getByRole("button", { name: "居中" });
    await user.click(center);
    expect(center).toHaveAttribute("aria-pressed", "true");
    expect(lead).toHaveStyle({ textAlign: "center" });

    const highlight = screen.getByRole("button", {
      name: "高亮：上传、处理、下载",
    });
    await user.click(highlight);
    expect(within(properties).getByRole("heading", { name: "注释属性" })).toBeInTheDocument();
    const note = within(properties).getByRole("textbox", { name: "注释内容" });
    expect(note).toHaveValue("确认“本机完成”的措辞与隐私承诺一致。");
    const opacity = within(properties).getByRole("slider", { name: "不透明度" });
    expect(opacity).toHaveValue("100");
    fireEvent.change(opacity, { target: { value: "40" } });
    expect(opacity).toHaveValue("40");
    const green = within(properties).getByRole("button", { name: "注释颜色：绿色" });
    await user.click(green);
    expect(green).toHaveAttribute("aria-pressed", "true");

    const rect = screen.getByRole("button", { name: "矩形：三个承诺" });
    await user.click(rect);
    expect(within(properties).getByRole("heading", { name: "形状属性" })).toBeInTheDocument();
    const fill = within(properties).getByRole("button", { name: "填充颜色：黄色" });
    await user.click(fill);
    expect(fill).toHaveAttribute("aria-pressed", "true");
    const strokeWidth = within(properties).getByRole("spinbutton", { name: "线宽" });
    expect(strokeWidth).toHaveValue(2);
    await user.clear(strokeWidth);
    await user.type(strokeWidth, "5");
    expect(strokeWidth).toHaveValue(5);

    await user.keyboard("{Escape}");
    expect(
      within(properties).queryByRole("heading", { name: "形状属性" }),
    ).not.toBeInTheDocument();
    expect(within(properties).getByRole("heading", { name: "注释工具" })).toBeInTheDocument();

    await user.click(lead);
    expect(within(properties).getByRole("heading", { name: "文本属性" })).toBeInTheDocument();
    const tabs = screen.getByRole("tablist", { name: "编辑工具页签" });
    await user.click(within(tabs).getByRole("tab", { name: "形状" }));
    expect(within(properties).getByRole("heading", { name: "形状工具" })).toBeInTheDocument();
  });

  it("collapses and expands the thumbnail rail and properties panel", async () => {
    const user = userEvent.setup();
    render(<PrototypeApp />);

    const rail = screen.getByRole("navigation", { name: "页面缩略图" });
    const collapseRail = within(rail).getByRole("button", { name: "收起缩略图" });
    expect(collapseRail).toHaveAttribute("aria-expanded", "true");
    await user.click(collapseRail);
    expect(
      screen.queryByRole("navigation", { name: "页面缩略图" }),
    ).not.toBeInTheDocument();
    const expandRail = screen.getByRole("button", { name: "展开缩略图" });
    expect(expandRail).toHaveAttribute("aria-expanded", "false");
    await user.click(expandRail);
    expect(screen.getByRole("navigation", { name: "页面缩略图" })).toBeInTheDocument();

    const properties = screen.getByRole("complementary", { name: "属性面板" });
    const collapseProperties = within(properties).getByRole("button", {
      name: "收起属性面板",
    });
    expect(collapseProperties).toHaveAttribute("aria-expanded", "true");
    await user.click(collapseProperties);
    expect(
      screen.queryByRole("complementary", { name: "属性面板" }),
    ).not.toBeInTheDocument();
    const expandProperties = screen.getByRole("button", { name: "展开属性面板" });
    expect(expandProperties).toHaveAttribute("aria-expanded", "false");
    await user.click(expandProperties);
    expect(
      screen.getByRole("complementary", { name: "属性面板" }),
    ).toBeInTheDocument();
  });

  it("supports keyboard tab navigation and annotation list navigation", async () => {
    const user = userEvent.setup();
    render(<PrototypeApp />);

    const tabs = screen.getByRole("tablist", { name: "编辑工具页签" });
    const annotateTab = within(tabs).getByRole("tab", { name: "注释" });
    annotateTab.focus();
    expect(annotateTab).toHaveFocus();

    await user.keyboard("{ArrowRight}");
    const shapeTab = within(tabs).getByRole("tab", { name: "形状" });
    expect(shapeTab).toHaveAttribute("aria-selected", "true");
    expect(shapeTab).toHaveFocus();

    await user.keyboard("{ArrowLeft}");
    expect(within(tabs).getByRole("tab", { name: "注释" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(within(tabs).getByRole("tab", { name: "注释" })).toHaveFocus();

    await user.keyboard("{End}");
    expect(within(tabs).getByRole("tab", { name: "表单" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(within(tabs).getByRole("tab", { name: "表单" })).toHaveFocus();

    await user.keyboard("{Home}");
    expect(within(tabs).getByRole("tab", { name: "注释" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(within(tabs).getByRole("tab", { name: "注释" })).toHaveFocus();

    const thumbnails = screen.getByRole("navigation", { name: "页面缩略图" });
    const fifthThumbnail = within(thumbnails).getByRole("button", {
      name: "第 5 页",
    });
    fifthThumbnail.focus();
    await user.keyboard("{Enter}");

    const statusBar = screen.getByRole("contentinfo");
    expect(
      within(statusBar).getByRole("spinbutton", { name: "当前页码" }),
    ).toHaveValue(5);

    const properties = screen.getByRole("complementary", { name: "属性面板" });
    const strikeoutItem = within(properties).getByRole("button", {
      name: /删除线：横向溢出检查/,
    });
    await user.click(strikeoutItem);
    expect(
      within(statusBar).getByRole("spinbutton", { name: "当前页码" }),
    ).toHaveValue(4);
    expect(
      within(properties).getByRole("heading", { name: "注释属性" }),
    ).toBeInTheDocument();
  });
});
