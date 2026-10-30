import { createRef } from "react";
import { render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { PageSurface } from "./PageViewer";
import type { PageRender } from "../lib/useDocumentViewer";

vi.mock("./PageTextLayer", () => ({ PageTextLayer: () => null }));
vi.mock("./PageImageLayer", () => ({ PageImageLayer: () => null }));
vi.mock("./FormLayer", () => ({ FormLayer: () => null }));
vi.mock("./ShapeLayer", () => ({ ShapeLayer: () => null }));
vi.mock("./PathShapeLayer", () => ({ PathShapeLayer: () => null }));
vi.mock("./FreeTextLayer", () => ({ FreeTextLayer: () => null }));
vi.mock("./LineLayer", () => ({ LineLayer: () => null }));
vi.mock("./LinkLayer", () => ({ LinkLayer: () => null }));
vi.mock("./StampLayer", () => ({ StampLayer: () => null }));
vi.mock("./EraserLayer", () => ({ EraserLayer: () => null }));

const props = {
  documentId: "doc", pageIndex: 0, scale: 2,
  info: {pageIndex: 0, width: 612, height: 792, rotation: 0 as const, cropX: 0, cropY: 0},
  readOnly: false, canAnnotate: false, annotations: [], selectedAnnotation: null,
  armedStamp: null, onSelectAnnotation: vi.fn(), onStampPlaced: vi.fn(),
  onArmedStampChange: vi.fn(), onAnnotationsChanged: vi.fn(), surfaceRef: createRef<HTMLDivElement>(),
};
const oldRender: PageRender = {
  scale: 1, pixelRatio: 2,
  image: {documentId: "doc", pageIndex: 0, scale: 2, width: 1224, height: 1584, pngBase64: "old"},
  regions: [],
};

it("paints the previous full page at the new zoom while native rendering is pending", () => {
  const {container, rerender} = render(<PageSurface {...props} render={oldRender} />);
  expect(screen.getByRole("region")).toHaveAttribute("data-render-mode", "preview");
  expect(container.querySelector(".page-image")).toHaveAttribute("src", "data:image/png;base64,old");
  expect(container.querySelector(".page-image")).toHaveStyle({width: "100%", height: "100%"});
  rerender(<PageSurface {...props} pageIndex={1} render={oldRender} />);
  expect(container.querySelector(".page-image")).toBeNull();
});

it("scales old tiles with the page and retains the full-page background behind a new tile", () => {
  const rendering = {...oldRender, regions: [{documentId: "doc", pageIndex: 0, scale: 2,
    x: 100, y: 200, width: 300, height: 400, pngBase64: "tile"}]};
  const {container} = render(<PageSurface {...props} scale={3} render={rendering} />);
  expect(container.querySelector(".page-image")).not.toBeNull();
  expect(container.querySelector(".page-region")).toHaveStyle({left: "150px", top: "300px", width: "450px", height: "600px"});
});
