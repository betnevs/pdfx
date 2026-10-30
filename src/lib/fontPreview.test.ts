import { describe, expect, it } from "vitest";

import { isFangSongFamily, isSerifFamily, previewFontFamily } from "./fontPreview";

describe("isSerifFamily", () => {
  it.each([
    "SimSun",
    "FangS-SC",
    "HYShuSongErKW",
    "Songti SC",
    "STSong",
    "MingLiU",
    "Times New Roman",
    "Georgia",
    "Cambria Math",
    "Noto Serif SC",
    "ZCOOL XiaoWei",
    "Ma Shan Zheng",
  ])("treats %s as a serif face", (family) => {
    expect(isSerifFamily(family)).toBe(true);
  });

  it.each([
    "Microsoft YaHei",
    "SimHei",
    "PingFang SC",
    "Helvetica Neue",
    "Arial",
    "Noto Sans SC",
    "Source Han Sans",
  ])("treats %s as a sans-serif face", (family) => {
    expect(isSerifFamily(family)).toBe(false);
  });

  it("lets a sans marker win over a serif-looking marker", () => {
    expect(isSerifFamily("Songti Sans")).toBe(false);
  });
});

describe("isFangSongFamily", () => {
  it.each(["FangS-SC", "STFangsong", "FangSong_GB2312", "仿宋_GB2312", "Zhuque Fangsong"])(
    "treats %s as a fangsong face",
    (family) => {
      expect(isFangSongFamily(family)).toBe(true);
    },
  );

  it.each(["SimSun", "Songti SC", "PingFang SC", "Microsoft YaHei", "FZLanTingHeiS-R-GB"])(
    "treats %s as not a fangsong face",
    (family) => {
      expect(isFangSongFamily(family)).toBe(false);
    },
  );
});

describe("previewFontFamily", () => {
  it("previews Song-style CJK fonts with Songti before any sans fallback", () => {
    const stack = previewFontFamily("SimSun");
    expect(stack).toContain("Songti SC");
    expect(stack.indexOf("Songti SC")).toBeLessThan(
      stack.indexOf("Noto Serif SC"),
    );
  });

  it("previews FangSong fonts with an installed fangsong face", () => {
    const stack = previewFontFamily("FangS-SC");
    expect(stack).toContain('"FangSong"');
    expect(stack).toContain("STFangsong");
    expect(stack.indexOf("STFangsong")).toBeLessThan(
      stack.indexOf("Songti SC"),
    );
  });

  it("previews unembedded WPS Song names as serif instead of the webview default", () => {
    expect(previewFontFamily("HYShuSongErKW")).toContain("Songti SC");
  });

  it("previews Kai faces with Kaiti", () => {
    expect(previewFontFamily("STKaiti")).toContain("Kaiti SC");
  });

  it("previews the bundled fallbacks with their own stacks", () => {
    expect(previewFontFamily("Noto Sans SC")).toContain("PingFang SC");
    expect(previewFontFamily("Noto Serif SC")).toContain("Songti SC");
  });

  it("previews the bundled display faces with the closest installed look", () => {
    expect(previewFontFamily("LXGW WenKai")).toContain("Kaiti SC");
    expect(previewFontFamily("Smiley Sans")).toContain("PingFang SC");
    expect(previewFontFamily("Ma Shan Zheng")).toContain("Songti SC");
    expect(previewFontFamily("ZCOOL XiaoWei")).toContain("Songti SC");
    expect(previewFontFamily("ZCOOL QingKe HuangYou")).toContain("PingFang SC");
  });

  it("previews Helvetica and Arial with a Latin sans stack", () => {
    const stack = previewFontFamily("Helvetica Neue");
    expect(stack.startsWith('"Helvetica Neue"')).toBe(true);
    expect(previewFontFamily("Arial")).toContain("Arial");
  });

  it("previews monospace faces with monospace", () => {
    expect(previewFontFamily("Courier New")).toContain("Courier New");
    expect(previewFontFamily("Courier")).toContain("monospace");
  });

  it("defaults unknown families to a CJK sans stack", () => {
    expect(previewFontFamily("FZLanTingHeiS-R-GB")).toContain("PingFang SC");
  });
});
