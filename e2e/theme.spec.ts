import { expect, test, type Page } from "@playwright/test";

/**
 * Theme behavior in a real browser engine:
 *
 * - the CSS media query repaints when the OS color scheme changes;
 * - a pinned choice is applied before first paint from the local cache;
 * - the PDF page stays paper-white and unfiltered in dark mode;
 * - panels and dialogs use the dark palette.
 *
 * Visual review of every surface remains manual; these checks cover the
 * observable contract, without screenshots.
 */

const LIGHT_BG = "rgb(244, 242, 238)";
const DARK_BG = "rgb(18, 21, 27)";
const LIGHT_SURFACE = "rgb(251, 250, 247)";
const DARK_SURFACE = "rgb(27, 31, 39)";
const DARK_RAISED = "rgb(36, 42, 53)";
const DARK_TEXT = "rgb(231, 233, 239)";
const DARK_MUTED = "rgb(166, 173, 187)";

async function rootTheme(page: Page) {
  return page.evaluate(() => ({
    theme: document.documentElement.dataset.theme ?? null,
    resolved: document.documentElement.dataset.resolvedTheme ?? null,
    background: getComputedStyle(document.documentElement).backgroundColor,
  }));
}

test("follows operating system color-scheme changes without a reload", async ({
  page,
}) => {
  await page.emulateMedia({ colorScheme: "light" });
  await page.goto("/");
  await expect(page.getByRole("heading", { level: 1, name: "PDFX" })).toBeVisible();

  await expect
    .poll(async () => rootTheme(page))
    .toEqual({ theme: null, resolved: "light", background: LIGHT_BG });
  await expect(page.locator(".app-header")).toHaveCSS(
    "background-color",
    LIGHT_SURFACE,
  );

  await page.emulateMedia({ colorScheme: "dark" });

  await expect
    .poll(async () => rootTheme(page))
    .toEqual({ theme: null, resolved: "dark", background: DARK_BG });
  await expect(page.locator(".app-header")).toHaveCSS(
    "background-color",
    DARK_SURFACE,
  );

  await page.emulateMedia({ colorScheme: "light" });

  await expect
    .poll(async () => rootTheme(page))
    .toEqual({ theme: null, resolved: "light", background: LIGHT_BG });
});

test("applies a pinned dark theme before first paint even on a light OS", async ({
  page,
}) => {
  await page.emulateMedia({ colorScheme: "light" });
  await page.addInitScript(() => {
    window.localStorage.setItem("pdfx.theme", "dark");
  });

  await page.goto("/", { waitUntil: "domcontentloaded" });

  // Checked at DOMContentLoaded: the module-level initializer must already
  // have applied the cached choice, so the first visible frame is dark.
  expect(await rootTheme(page)).toEqual({
    theme: "dark",
    resolved: "dark",
    background: DARK_BG,
  });

  // A pinned choice ignores the OS in both directions.
  await page.emulateMedia({ colorScheme: "light" });
  await expect
    .poll(async () => rootTheme(page))
    .toEqual({ theme: "dark", resolved: "dark", background: DARK_BG });
  await page.emulateMedia({ colorScheme: "dark" });
  await expect
    .poll(async () => rootTheme(page))
    .toEqual({ theme: "dark", resolved: "dark", background: DARK_BG });
});

test("applies a pinned light theme before first paint even on a dark OS", async ({
  page,
}) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await page.addInitScript(() => {
    window.localStorage.setItem("pdfx.theme", "light");
  });

  await page.goto("/", { waitUntil: "domcontentloaded" });

  expect(await rootTheme(page)).toEqual({
    theme: "light",
    resolved: "light",
    background: LIGHT_BG,
  });
});

test("keeps PDF page content paper-white and unfiltered in dark mode", async ({
  page,
}) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await page.addInitScript(() => {
    window.localStorage.setItem("pdfx.theme", "dark");
  });
  await page.goto("/");

  await page.evaluate(() => {
    const surface = document.createElement("div");
    surface.className = "page-surface";
    surface.style.width = "200px";
    surface.style.height = "260px";

    const image = document.createElement("img");
    image.className = "page-image";
    image.alt = "";
    surface.append(image);

    document.body.append(surface);
  });

  const pageSurface = await page.evaluate(() => {
    const surface = document.querySelector(".page-surface") as HTMLElement;
    const image = document.querySelector(".page-image") as HTMLElement;
    const describe = (element: HTMLElement) => {
      const styles = getComputedStyle(element);
      return {
        background: styles.backgroundColor,
        filter: styles.filter,
        mixBlendMode: styles.mixBlendMode,
      };
    };
    return {
      surface: describe(surface),
      image: describe(image),
      pageToken: getComputedStyle(document.documentElement)
        .getPropertyValue("--pdfx-page")
        .trim(),
    };
  });

  expect(pageSurface.pageToken).toBe("#ffffff");
  expect(pageSurface.surface).toEqual({
    background: "rgb(255, 255, 255)",
    filter: "none",
    mixBlendMode: "normal",
  });
  expect(pageSurface.image.filter).toBe("none");
  expect(pageSurface.image.mixBlendMode).toBe("normal");
});

test("paints the settings panel with the dark palette", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "light" });
  await page.addInitScript(() => {
    window.localStorage.setItem("pdfx.theme", "dark");
  });
  await page.goto("/");

  await page.getByRole("button", { name: "设置" }).click();

  const panel = page.getByRole("dialog", { name: "设置" });
  await expect(panel).toBeVisible();
  await expect(panel).toHaveCSS("background-color", DARK_SURFACE);
  await expect(panel).toHaveCSS("color", DARK_TEXT);

  const themeSelect = panel.getByLabel("主题");
  await expect(themeSelect).toHaveCSS("background-color", DARK_RAISED);
  await expect(themeSelect).toHaveCSS("color", DARK_TEXT);
  await expect(themeSelect).toHaveValue("system");

  await expect(panel.locator(".settings-hint").first()).toHaveCSS(
    "color",
    DARK_MUTED,
  );
  await expect(panel.locator(".settings-header")).toHaveCSS(
    "border-bottom-color",
    "rgb(49, 56, 70)",
  );
});
