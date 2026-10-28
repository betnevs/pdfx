import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

/**
 * CSS is read from disk because Vitest's `css: false` mode stubs CSS imports.
 * Parsing the real stylesheet keeps these tests independent of that setting.
 */
const themeCss = readFileSync(resolve(process.cwd(), "src/theme.css"), "utf8");
const stylesCss = readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8");
const settingsCss = readFileSync(resolve(process.cwd(), "src/settings.css"), "utf8");
const componentCss = [
  ["styles.css", stylesCss],
  ["settings.css", settingsCss],
] as const;

interface TokenMap {
  [name: string]: string;
}

interface TokenBlock {
  selector: string;
  tokens: TokenMap;
}

/** Removes comments so prose never trips the token and literal checks. */
function stripComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, "");
}

function parseTokenBlocks(css: string): TokenBlock[] {
  const blocks: TokenBlock[] = [];
  const rule = /([^{}]+)\{([^{}]*)\}/g;
  let match: RegExpExecArray | null;
  while ((match = rule.exec(stripComments(css))) !== null) {
    const selector = match[1].trim();
    const body = match[2];
    if (!body.includes("--pdfx-")) {
      continue;
    }
    const tokens: TokenMap = {};
    const token = /--pdfx-([\w-]+)\s*:\s*([^;]+);/g;
    let entry: RegExpExecArray | null;
    while ((entry = token.exec(body)) !== null) {
      tokens[entry[1]] = entry[2].trim();
    }
    blocks.push({ selector, tokens });
  }
  return blocks;
}

function collect(blocks: TokenBlock[], predicate: (selector: string) => boolean): TokenMap {
  return blocks
    .filter((block) => predicate(block.selector))
    .reduce<TokenMap>((tokens, block) => ({ ...tokens, ...block.tokens }), {});
}

function parseHexColor(color: string): [number, number, number] {
  const match = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(color.trim());
  if (!match) {
    throw new Error(`expected a hex color, received ${color}`);
  }
  const hex =
    match[1].length === 3
      ? match[1]
          .split("")
          .map((digit) => digit + digit)
          .join("")
      : match[1];
  return [0, 2, 4].map((offset) => Number.parseInt(hex.slice(offset, offset + 2), 16) / 255) as [
    number,
    number,
    number,
  ];
}

function relativeLuminance(color: string): number {
  const [red, green, blue] = parseHexColor(color).map((channel) =>
    channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4,
  );
  return 0.2126 * red + 0.7152 * green + 0.0722 * blue;
}

/** WCAG 2.x contrast ratio between two opaque hex colors. */
function contrast(foreground: string, background: string): number {
  const light = relativeLuminance(foreground);
  const dark = relativeLuminance(background);
  const [hi, lo] = light > dark ? [light, dark] : [dark, light];
  return (hi + 0.05) / (lo + 0.05);
}

const blocks = parseTokenBlocks(themeCss);
const light = collect(blocks, (selector) => selector === ":root");
const darkPinned = collect(blocks, (selector) => selector.includes('[data-theme="dark"]'));
const darkSystem = collect(blocks, (selector) => selector.includes(":not([data-theme])"));

/** Tokens the whole UI relies on; every theme must define each one. */
const REQUIRED_TOKENS = [
  "bg",
  "surface",
  "raised",
  "workspace",
  "dot",
  "page",
  "page-ink",
  "page-control-bg",
  "page-control-border",
  "page-danger-text",
  "placeholder",
  "overlay",
  "overlay-page",
  "text",
  "text-secondary",
  "muted",
  "disabled-text",
  "disabled-surface",
  "disabled-border",
  "border",
  "border-strong",
  "border-control",
  "border-hover",
  "focus-ring",
  "focus-halo",
  "accent",
  "accent-hover",
  "on-accent",
  "accent-soft",
  "accent-text",
  "danger-text",
  "danger-strong",
  "danger-soft",
  "danger-border",
  "warning-text",
  "warning-strong-text",
  "warning-soft",
  "warning-border",
  "blue",
  "blue-soft",
  "blue-text",
  "blue-ui",
  "blue-preview",
  "match-highlight",
  "pill-bg",
  "pill-bg-soft",
  "pill-text",
  "pill-border",
  "pill-hover",
  "pill-active",
  "status-success",
  "status-warning",
  "status-danger",
  "page-shadow",
  "thumbnail-shadow",
  "dialog-shadow",
  "popover-shadow",
  "settings-shadow",
  "editor-bg",
  "scrollbar-thumb",
  "scrollbar-track",
];

/** Pairs of [foreground, background] that carry normal-sized text. */
const TEXT_PAIRS: Array<[string, string]> = [
  ["text", "bg"],
  ["text", "surface"],
  ["text", "raised"],
  ["text", "workspace"],
  ["text", "danger-soft"],
  ["text", "warning-soft"],
  ["text-secondary", "surface"],
  ["text-secondary", "raised"],
  ["muted", "bg"],
  ["muted", "surface"],
  ["muted", "raised"],
  ["muted", "workspace"],
  ["accent-text", "accent-soft"],
  ["accent-text", "surface"],
  ["on-accent", "accent"],
  ["on-accent", "blue"],
  ["danger-text", "danger-soft"],
  ["danger-strong", "raised"],
  ["warning-text", "bg"],
  ["warning-text", "surface"],
  ["warning-strong-text", "warning-soft"],
  ["blue-text", "blue-soft"],
];

/** Pairs that identify controls and states rather than text (WCAG 3:1). */
const UI_PAIRS: Array<[string, string]> = [
  ["focus-ring", "bg"],
  ["focus-ring", "surface"],
  ["border-control", "raised"],
  ["border-control", "surface"],
  ["blue-ui", "raised"],
  ["blue-ui", "blue-soft"],
];

/** Text that sits on the white PDF page rather than UI chrome. */
const PAGE_TEXT_PAIRS: Array<[string, string]> = [
  ["page-ink", "page"],
  ["page-danger-text", "page"],
  ["on-accent", "blue"],
];

/** Outlines and controls drawn on top of the white PDF page. */
const PAGE_UI_PAIRS: Array<[string, string]> = [
  ["page-control-border", "page-control-bg"],
  ["blue", "page"],
];

const THEMES: Array<[string, TokenMap]> = [
  ["light", light],
  ["dark (pinned)", darkPinned],
  ["dark (system)", darkSystem],
];

describe("theme tokens", () => {
  it("defines the complete token set in light, pinned dark and system dark", () => {
    expect(blocks.length).toBeGreaterThan(0);
    for (const [theme, tokens] of THEMES) {
      for (const name of REQUIRED_TOKENS) {
        expect(tokens[name], `--pdfx-${name} is missing from ${theme}`).toBeDefined();
      }
    }
  });

  it("keeps every theme's token names in sync", () => {
    const lightNames = Object.keys(light).sort();
    expect(Object.keys(darkPinned).sort()).toEqual(lightNames);
    expect(Object.keys(darkSystem).sort()).toEqual(lightNames);
  });

  it("declares the matching color-scheme and a system-dark media query", () => {
    expect(stripComments(themeCss)).toMatch(
      /@media\s*\(prefers-color-scheme:\s*dark\)\s*\{[\s\S]*?:root:not\(\[data-theme\]\)/,
    );
    expect(themeCss).toMatch(/:root\s*\{[^}]*color-scheme:\s*light;/);
    expect(themeCss).toMatch(
      /:root\[data-theme="dark"\][^{]*\{[^}]*color-scheme:\s*dark;/,
    );
    expect(themeCss).toMatch(
      /:root:not\(\[data-theme\]\)\s*\{[^}]*color-scheme:\s*dark;/,
    );
  });

  it("themes scrollbars instead of leaving them light on dark surfaces", () => {
    expect(themeCss).toMatch(/scrollbar-color:\s*var\(--pdfx-scrollbar-thumb\)/);
    expect(themeCss).toMatch(/::-webkit-scrollbar-thumb\s*\{[^}]*var\(--pdfx-scrollbar-thumb\)/);
  });

  it("never inverts the PDF page and keeps it paper-white in every theme", () => {
    for (const [theme, tokens] of THEMES) {
      expect(tokens.page, `${theme} page background`).toBe("#ffffff");
      expect(tokens["page-ink"], `${theme} page ink`).toBe("#1f2430");
      expect(tokens["page-control-bg"], `${theme} page control background`).toBe("#ffffff");
    }
    expect(themeCss).not.toMatch(/filter\s*:\s*(?!none\b)[^;]+;/i);
    expect(themeCss).not.toMatch(/mix-blend-mode\s*:\s*(?!normal\b)[^;]+;/i);
  });
});

describe.each(THEMES)("%s contrast", (_theme, tokens) => {
  it("keeps text readable on every surface", () => {
    for (const [foreground, background] of TEXT_PAIRS) {
      const ratio = contrast(tokens[foreground], tokens[background]);
      expect(
        ratio,
        `--pdfx-${foreground} on --pdfx-${background} should reach 4.5:1`,
      ).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("keeps control boundaries, focus rings and selected states recognisable", () => {
    for (const [foreground, background] of UI_PAIRS) {
      const ratio = contrast(tokens[foreground], tokens[background]);
      expect(
        ratio,
        `--pdfx-${foreground} on --pdfx-${background} should reach 3:1`,
      ).toBeGreaterThanOrEqual(3);
    }
  });

  it("keeps page text readable on the white PDF page", () => {
    for (const [foreground, background] of PAGE_TEXT_PAIRS) {
      const ratio = contrast(tokens[foreground], tokens[background]);
      expect(
        ratio,
        `--pdfx-${foreground} on --pdfx-${background} should reach 4.5:1`,
      ).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("keeps page overlays recognisable on the white PDF page", () => {
    for (const [foreground, background] of PAGE_UI_PAIRS) {
      const ratio = contrast(tokens[foreground], tokens[background]);
      expect(
        ratio,
        `--pdfx-${foreground} on --pdfx-${background} should reach 3:1`,
      ).toBeGreaterThanOrEqual(3);
    }
  });
});

describe.each(componentCss)("%s theming", (_file, css) => {
  it("uses theme tokens instead of hardcoded colors", () => {
    const withoutComments = stripComments(css);
    expect(withoutComments).not.toMatch(/#[0-9a-f]{3,8}\b/i);
    expect(withoutComments).not.toMatch(/\b(?:rgba?|hsla?)\(/i);
  });

  it("never inverts or blends the PDF page", () => {
    expect(css).not.toMatch(/filter\s*:\s*(?!none\b)[^;]+;/i);
    expect(css).not.toMatch(/mix-blend-mode\s*:\s*(?!normal\b)[^;]+;/i);
  });
});

describe("theme token references", () => {
  it("only references tokens that every theme defines", () => {
    const defined = new Set(Object.keys(light));
    const used = new Set<string>();
    for (const css of [themeCss, stylesCss, settingsCss]) {
      for (const match of stripComments(css).matchAll(/var\(--pdfx-([\w-]+)/g)) {
        used.add(match[1]);
      }
    }
    for (const name of used) {
      expect(defined.has(name), `--pdfx-${name} is not defined in :root`).toBe(true);
    }
  });
});
