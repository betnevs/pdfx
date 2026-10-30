/**
 * CSS font stacks for previewing PDF fonts in the webview.
 *
 * A PDF text object names the font program it embeds (`FangS-SC`,
 * `HYShuSongErKW`, `ArialUnicodeMS`, ...). The webview cannot resolve those
 * names, so using the raw name in the inline editor falls back to its default
 * sans-serif face and a Song-style document appears to change typeface before
 * the edit is even applied. These stacks map the naming used by Word, WPS and
 * Acrobat to the faces macOS and Windows install.
 */

const CJK_SANS = '"PingFang SC", "Microsoft YaHei", "Noto Sans SC", "Helvetica Neue", Arial, sans-serif';
const CJK_SERIF = '"Songti SC", STSong, SimSun, "Noto Serif SC", "Times New Roman", serif';
const CJK_FANGSONG =
  '"FangSong", "STFangsong", "FangSong_GB2312", "Songti SC", STSong, "Times New Roman", serif';
const CJK_KAI = '"Kaiti SC", KaiTi, STKaiti, "Songti SC", "Noto Serif SC", serif';
const LATIN_SANS = '"Helvetica Neue", Helvetica, Arial, "PingFang SC", "Microsoft YaHei", sans-serif';
const MONO = '"Courier New", Courier, "SFMono-Regular", Menlo, monospace';

/** Sans markers checked first, because they also occur inside serif names. */
const SANS_MARKERS = ["sans", "pingfang", "hei"];

/** Serif markers shared with the backend's fallback-family choice. */
const SERIF_MARKERS = [
  "song",
  "sun",
  "ming",
  "fangsong",
  "fang",
  "serif",
  "times",
  "georgia",
  "cambria",
  "roman",
  "xiaowei",
  "shanzheng",
];

/** True when the family name looks like a Song/Ming/FangSong/Times face. */
export function isSerifFamily(family: string): boolean {
  const name = compact(family);
  // "PingFang" and "Hei" are sans faces even though "fang" also appears in
  // FangSong; check them before the serif markers so PingFang SC never
  // previews as a Song face.
  if (SANS_MARKERS.some((marker) => name.includes(marker))) {
    return false;
  }
  return SERIF_MARKERS.some((marker) => name.includes(marker));
}

/**
 * True when the family name looks like a FangSong face (FangS-SC, STFangsong,
 * FangSong_GB2312, 仿宋). Mirrors the backend's fallback-family choice: the
 * name must contain the full "fangsong" stem or "FangS-SC"'s compact form, so
 * 方正 (FangZheng) Song faces are not mistaken for fangsong.
 */
export function isFangSongFamily(family: string): boolean {
  if (family.includes("仿宋")) {
    return true;
  }
  const name = compact(family);
  return name.includes("fangsong") || name.includes("fangssc");
}

/** The CSS family list to preview one PDF font with. */
export function previewFontFamily(family: string): string {
  const name = compact(family);
  if (name.includes("courier") || name.includes("mono")) {
    return MONO;
  }
  if (name.includes("notosanssc")) {
    return CJK_SANS;
  }
  if (name.includes("notoserifsc")) {
    return CJK_SERIF;
  }
  if (isFangSongFamily(family)) {
    return CJK_FANGSONG;
  }
  if (name.includes("kai")) {
    return CJK_KAI;
  }
  if (isSerifFamily(family)) {
    return CJK_SERIF;
  }
  if (name.includes("helvetica") || name.includes("arial") || name.includes("verdana")) {
    return LATIN_SANS;
  }
  if (name.includes("sans")) {
    return CJK_SANS;
  }
  // Unknown families are usually CJK faces from Word/WPS exports; a CJK stack
  // still renders Latin text through PingFang's Latin glyphs.
  return CJK_SANS;
}

function compact(family: string): string {
  return family.toLowerCase().replace(/[^a-z0-9]/g, "");
}
