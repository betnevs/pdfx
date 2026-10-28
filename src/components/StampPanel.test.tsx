import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { StampPanel } from "./StampPanel";

describe("StampPanel", () => {
  it("offers the four built-in stamps with their Chinese labels", () => {
    render(
      <StampPanel
        armed={null}
        canAnnotate
        readOnly={false}
        onPick={vi.fn()}
      />,
    );

    for (const label of ["已批准", "草稿", "机密", "已拒绝"]) {
      expect(
        screen.getByRole("button", { name: `放置${label}图章` }),
      ).toBeInTheDocument();
      expect(screen.getByText(label)).toBeInTheDocument();
    }
    expect(
      screen.queryByRole("status"),
    ).not.toBeInTheDocument();
  });

  it("arms the picked stamp, shows the placement hint and can cancel", () => {
    const onPick = vi.fn();
    const { rerender } = render(
      <StampPanel
        armed={null}
        canAnnotate
        readOnly={false}
        onPick={onPick}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "放置机密图章" }));
    expect(onPick).toHaveBeenCalledWith("confidential");

    rerender(
      <StampPanel
        armed="confidential"
        canAnnotate
        readOnly={false}
        onPick={onPick}
      />,
    );
    expect(
      screen.getByRole("button", { name: "放置机密图章" }),
    ).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("status")).toHaveTextContent(
      "在页面上单击放置“机密”图章。",
    );

    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(onPick).toHaveBeenLastCalledWith(null);

    fireEvent.click(screen.getByRole("button", { name: "放置机密图章" }));
    expect(onPick).toHaveBeenLastCalledWith(null);
  });

  it("disables placement when annotations are not allowed", () => {
    render(
      <StampPanel
        armed={null}
        canAnnotate={false}
        readOnly={false}
        onPick={vi.fn()}
      />,
    );

    expect(screen.getByRole("button", { name: "放置已批准图章" })).toBeDisabled();
    expect(screen.getByText("此文档不允许添加图章")).toBeInTheDocument();

    cleanup();
    render(
      <StampPanel armed={null} canAnnotate readOnly onPick={vi.fn()} />,
    );
    expect(screen.getByRole("button", { name: "放置已批准图章" })).toBeDisabled();
  });
});
