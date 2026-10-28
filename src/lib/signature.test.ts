import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  addStrokePoint,
  drawingBounds,
  encodePng,
  hasTransparency,
  hasVisiblePixels,
  paintSignatureStrokes,
  pngDataUrlToBase64,
  processUploadedImage,
  removeWhiteBackground,
  renderDrawingPng,
  renderTypedPng,
  SIGNATURE_MAX_FILE_BYTES,
  trimTransparent,
  uploadedFileError,
  uploadedFormat,
  validateSignatureName,
  whiteBackgroundRatio,
  type RgbaImage,
} from "./signature";

function rgba(width: number, height: number, fill: [number, number, number, number]) {
  const data = new Uint8ClampedArray(width * height * 4);
  for (let offset = 0; offset < data.length; offset += 4) {
    data.set(fill, offset);
  }
  return { width, height, data };
}

function setPixel(
  image: RgbaImage,
  x: number,
  y: number,
  colour: [number, number, number, number],
) {
  image.data.set(colour, (y * image.width + x) * 4);
}

describe("validateSignatureName", () => {
  it("accepts a trimmed Chinese name", () => {
    expect(validateSignatureName("  张三  ")).toBeNull();
  });

  it("rejects blank, overlong and control-character names", () => {
    expect(validateSignatureName("   ")).toBe("请输入要签写的姓名。");
    expect(validateSignatureName("名".repeat(81))).toBe(
      "姓名不能超过 80 个字符。",
    );
    expect(validateSignatureName("签\n名")).toBe(
      "姓名不能包含换行或控制字符。",
    );
    expect(validateSignatureName("名".repeat(80))).toBeNull();
  });
});

describe("pngDataUrlToBase64", () => {
  it("strips the canvas PNG prefix", () => {
    expect(pngDataUrlToBase64("data:image/png;base64,QUJD")).toBe("QUJD");
  });

  it("rejects other data URLs", () => {
    expect(pngDataUrlToBase64("data:image/jpeg;base64,QUJD")).toBeNull();
    expect(pngDataUrlToBase64("https://example.com/a.png")).toBeNull();
  });
});

describe("stroke helpers", () => {
  it("drops points closer than the minimum distance", () => {
    const start = [{ x: 10, y: 10 }];
    expect(addStrokePoint(start, { x: 10.5, y: 10.5 })).toBe(start);
    expect(addStrokePoint(start, { x: 20, y: 20 })).toEqual([
      { x: 10, y: 10 },
      { x: 20, y: 20 },
    ]);
  });

  it("computes a padded bound and rejects an empty drawing", () => {
    expect(drawingBounds([])).toBeNull();
    expect(
      drawingBounds(
        [
          [
            { x: 100, y: 200 },
            { x: 300, y: 260 },
          ],
        ],
        1600,
        600,
      ),
    ).toEqual({ x: 90, y: 190, width: 220, height: 80 });
    expect(
      drawingBounds([[{ x: 2, y: 2 }]], 1600, 600),
    ).toEqual({ x: 0, y: 0, width: 12, height: 12 });
  });
});

class FakeContext {
  calls: string[] = [];
  font = "";
  strokeStyle = "";
  fillStyle = "";
  lineWidth = 1;
  lineCap = "";
  lineJoin = "";
  textAlign = "";
  textBaseline = "";

  clearRect(): void {
    this.calls.push("clearRect");
  }
  beginPath(): void {
    this.calls.push("beginPath");
  }
  moveTo(): void {
    this.calls.push("moveTo");
  }
  lineTo(): void {
    this.calls.push("lineTo");
  }
  stroke(): void {
    this.calls.push("stroke");
  }
  arc(): void {
    this.calls.push("arc");
  }
  fill(): void {
    this.calls.push("fill");
  }
  fillRect(): void {
    this.calls.push("fillRect");
  }
  translate(): void {
    this.calls.push("translate");
  }
  fillText(text: string): void {
    this.calls.push(`fillText:${text}`);
  }
  measureText(text: string): { width: number } {
    const size = Number.parseFloat(this.font) || 16;
    return { width: [...text].length * size };
  }
  createImageData(width: number, height: number) {
    return { width, height, data: new Uint8ClampedArray(width * height * 4) };
  }
  putImageData(): void {
    this.calls.push("putImageData");
  }
}

describe("canvas rendering", () => {
  const contexts = new Map<HTMLCanvasElement, FakeContext>();
  const originalGetContext = HTMLCanvasElement.prototype.getContext;
  const originalToDataURL = HTMLCanvasElement.prototype.toDataURL;

  beforeEach(() => {
    contexts.clear();
    HTMLCanvasElement.prototype.getContext = function (
      this: HTMLCanvasElement,
      kind: string,
    ) {
      if (kind !== "2d") {
        return null;
      }
      const context = new FakeContext();
      contexts.set(this, context);
      return context;
    } as unknown as typeof HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.toDataURL = () =>
      "data:image/png;base64,QUJD";
  });

  afterEach(() => {
    HTMLCanvasElement.prototype.getContext = originalGetContext;
    HTMLCanvasElement.prototype.toDataURL = originalToDataURL;
  });

  it("paints multi-point strokes and dots on a cleared transparent canvas", () => {
    const ctx = new FakeContext();
    paintSignatureStrokes(
      ctx as unknown as CanvasRenderingContext2D,
      [[{ x: 1, y: 2 }, { x: 3, y: 4 }], [{ x: 5, y: 6 }]],
      1600,
      600,
    );
    expect(ctx.calls[0]).toBe("clearRect");
    expect(ctx.calls).toContain("moveTo");
    expect(ctx.calls).toContain("lineTo");
    expect(ctx.calls).toContain("stroke");
    expect(ctx.calls).toContain("arc");
    expect(ctx.calls).toContain("fill");
    expect(ctx.calls).not.toContain("fillRect");
    expect(ctx.lineWidth).toBe(10);
  });

  it("renders a cropped drawing without a background fill", () => {
    const rendered = renderDrawingPng([
      [
        { x: 100, y: 100 },
        { x: 400, y: 300 },
      ],
    ]);
    expect(rendered).toEqual({
      dataUrl: "data:image/png;base64,QUJD",
      base64: "QUJD",
    });
    const contextsUsed = [...contexts.values()];
    expect(contextsUsed).toHaveLength(1);
    expect(contextsUsed[0].calls).not.toContain("fillRect");
    expect(contextsUsed[0].calls).toContain("stroke");
  });

  it("returns null for an empty drawing", () => {
    expect(renderDrawingPng([])).toBeNull();
  });

  it("renders typed text with the bundled face and no background", () => {
    const rendered = renderTypedPng("张三");
    expect(rendered?.base64).toBe("QUJD");
    const used = [...contexts.values()];
    expect(used).toHaveLength(2);
    expect(used[1].calls).toContain("fillText:张三");
    expect(used[1].calls).not.toContain("fillRect");
    expect(used[1].font).toContain("Ma Shan Zheng");
  });

  it("returns null for a blank typed name", () => {
    expect(renderTypedPng("   ")).toBeNull();
  });

  it("encodes RGBA pixels through putImageData", () => {
    const encoded = encodePng(rgba(2, 2, [10, 20, 30, 255]));
    expect(encoded?.base64).toBe("QUJD");
    expect([...contexts.values()][0].calls).toContain("putImageData");
  });
});

describe("uploaded pixels", () => {
  function whitePageWithInk(): RgbaImage {
    const image = rgba(21, 21, [255, 255, 255, 255]);
    setPixel(image, 10, 10, [20, 20, 20, 255]);
    return image;
  }

  it("detects transparency and near-white backgrounds", () => {
    const opaque = whitePageWithInk();
    expect(hasTransparency(opaque)).toBe(false);
    expect(whiteBackgroundRatio(opaque)).toBeCloseTo(440 / 441);

    const transparent = rgba(2, 2, [0, 0, 0, 0]);
    expect(hasTransparency(transparent)).toBe(true);
    expect(hasVisiblePixels(transparent)).toBe(false);
  });

  it("makes white pixels transparent and keeps the ink", () => {
    const cleared = removeWhiteBackground(whitePageWithInk());
    expect(cleared.data[(10 * 21 + 10) * 4 + 3]).toBe(255);
    expect(cleared.data[3]).toBe(0);
    expect(hasVisiblePixels(trimTransparent(cleared) ?? cleared)).toBe(true);
  });

  it("trims transparent margins with padding", () => {
    const image = rgba(6, 6, [0, 0, 0, 0]);
    setPixel(image, 2, 2, [10, 10, 10, 255]);
    setPixel(image, 3, 3, [10, 10, 10, 255]);
    const trimmed = trimTransparent(image, 1);
    expect(trimmed).not.toBeNull();
    expect(trimmed?.width).toBe(4);
    expect(trimmed?.height).toBe(4);
    expect(trimmed?.data[(1 * 4 + 1) * 4 + 3]).toBe(255);
  });

  it("returns null when every pixel is transparent", () => {
    expect(trimTransparent(rgba(3, 3, [0, 0, 0, 0]))).toBeNull();
  });

  it("keeps opaque uploads untouched when removal is off", () => {
    const original = whitePageWithInk();
    const processed = processUploadedImage(original, false);
    expect(processed.image).toBe(original);
    expect(processed.removedBackground).toBe(false);
    expect(processed.trimmed).toBe(false);
  });

  it("removes the background and trims when removal is on", () => {
    const processed = processUploadedImage(whitePageWithInk(), true);
    expect(processed.removedBackground).toBe(true);
    expect(processed.trimmed).toBe(true);
    expect(processed.image.width).toBeLessThan(21);
    expect(hasVisiblePixels(processed.image)).toBe(true);
  });
});

describe("uploaded files", () => {
  const pngHead = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  const jpegHead = new Uint8Array([0xff, 0xd8, 0xff, 0xe0, 0, 0, 0, 0]);

  it("detects PNG and JPEG from their magic bytes", async () => {
    expect(await uploadedFormat(new File([pngHead], "a.bin"))).toBe("png");
    expect(await uploadedFormat(new File([jpegHead], "b.bin"))).toBe("jpeg");
    expect(await uploadedFormat(new File(["plain text"], "c.txt"))).toBeNull();
  });

  it("rejects oversized and non-image files with Chinese copy", async () => {
    const oversized = {
      size: SIGNATURE_MAX_FILE_BYTES + 1,
      slice: () => new Blob(),
    } as unknown as File;
    expect(await uploadedFileError(oversized)).toBe(
      "图片过大：文件不能超过 25 MB，像素不能超过 2000 万。",
    );
    expect(await uploadedFileError(new File(["plain text"], "notes.txt"))).toBe(
      "仅支持 PNG 和 JPG 格式的图片。",
    );
    expect(await uploadedFileError(new File([pngHead], "signature.png"))).toBeNull();
  });
});
