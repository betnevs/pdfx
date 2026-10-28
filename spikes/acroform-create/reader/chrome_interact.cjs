// Drives the installed Chrome PDF viewer: clicks the full_name text field,
// appends text, and toggles the subscribe checkbox. This proves the created
// widgets are interactive, not only painted.
//
// Run:
//   NODE_PATH=/Users/nevs/Projects/pdfx/node_modules \
//     node reader/chrome_interact.cjs fixtures/all-fields.pdf out.png
const { chromium } = require('playwright-core');
const path = require('path');

(async () => {
  const input = process.argv[2];
  const output = process.argv[3];
  if (!input || !output) {
    console.error('usage: chrome_interact.cjs file.pdf screenshot.png');
    process.exit(2);
  }

  const browser = await chromium.launch({ channel: 'chrome', headless: true });
  const page = await browser.newPage({ viewport: { width: 1400, height: 1000 } });
  await page.goto('file://' + path.resolve(input));
  await page.waitForTimeout(2500);

  // Measured from viewer.viewport_: content 826x1066 inside a 1099x944 plugin
  // at (301,56), with a 612x792 page. These constants are specific to the
  // all-fields fixture and this viewport.
  const scale = 826 / 612;
  const left = 301 + (1099 - 826) / 2;
  const top = 56;
  const screenX = (x) => left + x * scale;
  const screenY = (y) => top + (792 - y) * scale;

  // full_name rect [72 700 320 724]; subscribe rect [72 600 88 616].
  await page.mouse.click(screenX(196), screenY(712));
  await page.keyboard.press('End');
  await page.keyboard.type(' - Chrome');
  await page.waitForTimeout(400);
  await page.mouse.click(screenX(80), screenY(608));

  await page.waitForTimeout(1000);
  await page.screenshot({ path: output });
  await browser.close();
})();
