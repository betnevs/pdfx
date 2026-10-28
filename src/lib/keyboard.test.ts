import { describe, expect, it } from "vitest";

import { isEditableTarget } from "./keyboard";

describe("isEditableTarget", () => {
  it("recognizes text inputs, textareas and selects", () => {
    const input = document.createElement("input");
    const textarea = document.createElement("textarea");
    const select = document.createElement("select");

    expect(isEditableTarget(input)).toBe(true);
    expect(isEditableTarget(textarea)).toBe(true);
    expect(isEditableTarget(select)).toBe(true);
  });

  it("recognizes contenteditable hosts and their descendants", () => {
    const host = document.createElement("div");
    host.setAttribute("contenteditable", "true");
    const child = document.createElement("span");
    host.appendChild(child);

    expect(isEditableTarget(host)).toBe(true);
    expect(isEditableTarget(child)).toBe(true);
  });

  it("ignores non-editable elements and non-elements", () => {
    expect(isEditableTarget(document.createElement("div"))).toBe(false);
    expect(isEditableTarget(document.createElement("button"))).toBe(false);
    expect(isEditableTarget(document.body)).toBe(false);
    expect(isEditableTarget(null)).toBe(false);
    expect(isEditableTarget(new EventTarget())).toBe(false);
  });
});
