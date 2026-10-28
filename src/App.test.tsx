import { render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";

import { App } from "./App";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("PDFX workspace", () => {
  afterEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("shows the empty workspace and the app version from the backend", async () => {
    vi.mocked(invoke).mockResolvedValue({ name: "PDFX", version: "9.8.7" });

    render(<App />);

    expect(screen.getByRole("heading", { level: 1, name: "PDFX" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 2, name: "空工作区" })).toBeInTheDocument();
    expect(
      screen.getByText("打开一个 PDF 文件开始编辑。文件只在本机处理。"),
    ).toBeInTheDocument();
    expect(await screen.findByRole("status")).toHaveTextContent("PDFX 9.8.7");
  });

  it("keeps the empty workspace visible when app info is unavailable", async () => {
    vi.mocked(invoke).mockRejectedValue(new Error("not running inside Tauri"));

    render(<App />);

    expect(await screen.findByRole("status")).toHaveTextContent("无法获取应用信息");
    expect(screen.getByRole("heading", { level: 2, name: "空工作区" })).toBeInTheDocument();
  });
});
