import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { TextBoxFontView, TextBoxTargetView } from "../lib/backend";
import { TextBoxStylePanel } from "./TextBoxStylePanel";

function font(
  overrides: Partial<TextBoxFontView> &
    Pick<TextBoxFontView, "family" | "label" | "source">,
): TextBoxFontView {
  return {
    bold: false,
    italic: false,
    postscriptName: overrides.family,
    embedded: false,
    embeddable: true,
    coverage: "system",
    ...overrides,
  };
}

const FONTS: TextBoxFontView[] = [
  font({
    family: "Helvetica",
    label: "Helvetica",
    postscriptName: "Helvetica",
    source: "base14",
    coverage: "win_ansi",
  }),
  font({
    family: "Noto Sans SC",
    label: "Noto Sans SC",
    postscriptName: "NotoSansSC",
    source: "embedded",
    embedded: true,
    coverage: "cjk",
  }),
  font({
    family: "FangS-SC",
    label: "仿宋-简",
    postscriptName: "FangS-SC",
    source: "wps",
  }),
  font({
    family: "ShuS-SC",
    label: "书宋-简",
    postscriptName: "ShuS-SC",
    source: "wps",
  }),
  font({ family: "Georgia", label: "Georgia", source: "system" }),
];

function target(overrides: Partial<TextBoxTargetView> = {}): TextBoxTargetView {
  return {
    boxId: 8,
    pageIndex: 0,
    text: "Original",
    fontFamily: "FangS-SC",
    fontSize: 18,
    color: "#000000",
    x: 72,
    y: 72,
    width: 58,
    height: 20,
    bold: false,
    italic: false,
    underline: false,
    alignment: "left",
    rotation: 0,
    autoHeight: true,
    created: false,
    keepOriginalFont: true,
    runs: [],
    ...overrides,
  };
}

describe("TextBoxStylePanel font groups", () => {
  it("groups bundled, WPS and installed fonts and keeps one original choice", () => {
    render(
      <TextBoxStylePanel
        target={target()}
        fonts={FONTS}
        busy={false}
        onChange={vi.fn()}
        onDelete={vi.fn()}
      />,
    );

    const select = screen.getByRole("combobox", { name: "字体" });
    expect((select as HTMLSelectElement).value).toBe("__original__");
    expect(
      screen.getAllByRole("option", { name: "原字体 · 仿宋-简" }),
    ).toHaveLength(1);

    const bundled = screen.getByRole("option", { name: "Noto Sans SC" });
    expect(bundled.closest("optgroup")).toHaveAttribute("label", "内置字体");

    const wps = screen.getByRole("option", { name: "书宋-简" });
    expect(wps.closest("optgroup")).toHaveAttribute("label", "WPS 字体");

    const installed = screen.getByRole("option", { name: "Georgia" });
    expect(installed.closest("optgroup")).toHaveAttribute(
      "label",
      "本机字体",
    );

    // The installed copy of the family being kept is not offered: its option
    // value equals the original family, so choosing it would change nothing.
    expect(
      screen.queryByRole("option", { name: "仿宋-简" }),
    ).not.toBeInTheDocument();
  });

  it("sends the original family for the original choice and unlocks other fonts", () => {
    const onChange = vi.fn();
    render(
      <TextBoxStylePanel
        target={target()}
        fonts={FONTS}
        busy={false}
        onChange={onChange}
        onDelete={vi.fn()}
      />,
    );

    const select = screen.getByRole("combobox", { name: "字体" });
    fireEvent.change(select, { target: { value: "Georgia" } });
    expect(onChange).toHaveBeenLastCalledWith(
      expect.objectContaining({ fontFamily: "Georgia" }),
      null,
    );

    fireEvent.change(select, { target: { value: "__original__" } });
    expect(onChange).toHaveBeenLastCalledWith(
      expect.objectContaining({ fontFamily: "FangS-SC" }),
      null,
    );
  });

  it("lists the installed family when the box no longer keeps its original font", () => {
    render(
      <TextBoxStylePanel
        target={target({ keepOriginalFont: false, created: true })}
        fonts={FONTS}
        busy={false}
        onChange={vi.fn()}
        onDelete={vi.fn()}
      />,
    );

    const installed = screen.getByRole("option", { name: "仿宋-简" });
    expect(installed.closest("optgroup")).toHaveAttribute("label", "WPS 字体");
  });
});
