import { expect, test } from "@playwright/test";

test("workspace fits the viewport and core controls stay in sync", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error" && !/favicon/i.test(message.text())) {
      errors.push(message.text());
    }
  });

  await page.goto("/prototype.html");
  await expect(page.getByRole("heading", { level: 1, name: "PDFX" })).toBeVisible();
  await expect(page.getByRole("tablist", { name: "编辑工具页签" })).toBeVisible();
  await expect(page.getByRole("main", { name: "页面画布" })).toBeVisible();
  await expect(
    page.getByRole("complementary", { name: "属性面板" }),
  ).toBeVisible();

  const viewport = page.viewportSize();
  expect(viewport).not.toBeNull();

  const metrics = await page.evaluate(() => ({
    documentScrollWidth: document.documentElement.scrollWidth,
    documentClientWidth: document.documentElement.clientWidth,
    documentScrollHeight: document.documentElement.scrollHeight,
    documentClientHeight: document.documentElement.clientHeight,
    bodyScrollWidth: document.body.scrollWidth,
    bodyScrollHeight: document.body.scrollHeight,
  }));
  expect(metrics.documentScrollWidth).toBeLessThanOrEqual(
    metrics.documentClientWidth,
  );
  expect(metrics.documentScrollHeight).toBeLessThanOrEqual(
    metrics.documentClientHeight,
  );
  expect(metrics.bodyScrollWidth).toBeLessThanOrEqual(
    metrics.documentClientWidth,
  );
  expect(metrics.bodyScrollHeight).toBeLessThanOrEqual(
    metrics.documentClientHeight,
  );

  for (const selector of [
    ".proto-header",
    ".proto-toolbar",
    ".thumbnail-rail",
    ".canvas-region",
    ".properties-panel",
    ".status-bar",
  ]) {
    const box = await page.locator(selector).boundingBox();
    expect(box, `${selector} should have a box`).not.toBeNull();
    expect(box!.x).toBeGreaterThanOrEqual(0);
    expect(box!.y).toBeGreaterThanOrEqual(0);
    expect(box!.x + box!.width).toBeLessThanOrEqual(viewport!.width + 1);
    expect(box!.y + box!.height).toBeLessThanOrEqual(viewport!.height + 1);
  }

  await page.getByRole("button", { name: "收起缩略图" }).click();
  await expect(
    page.getByRole("navigation", { name: "页面缩略图" }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "展开缩略图" }).click();
  await expect(
    page.getByRole("navigation", { name: "页面缩略图" }),
  ).toBeVisible();

  await page.getByRole("button", { name: "收起属性面板" }).click();
  await expect(
    page.getByRole("complementary", { name: "属性面板" }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "展开属性面板" }).click();
  await expect(
    page.getByRole("complementary", { name: "属性面板" }),
  ).toBeVisible();

  await page.getByRole("tab", { name: "形状" }).click();
  await expect(page.getByRole("heading", { name: "形状工具" })).toBeVisible();
  await page.getByRole("button", { name: "第 3 页" }).click();
  await expect(
    page.getByRole("spinbutton", { name: "当前页码" }),
  ).toHaveValue("3");
  await expect(
    page.getByRole("heading", { name: "文件从不离开这台设备" }),
  ).toBeVisible();

  await page.getByRole("button", { name: "适合页面" }).click();
  await expect(page.getByRole("button", { name: "适合页面" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );

  expect(errors).toEqual([]);
});
