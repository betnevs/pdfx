// Opens a PDF in installed Google Chrome via Playwright and captures a
// screenshot. Run with:
//   NODE_PATH=/Users/nevs/Projects/pdfx/node_modules node reader/chrome_probe.cjs file.pdf out.png
const { chromium } = require('playwright-core');
const path = require('path');

(async () => {
  const pdf = path.resolve(process.argv[2]);
  const out = path.resolve(process.argv[3]);
  const browser = await chromium.launch({ channel: 'chrome', headless: true });
  const page = await browser.newPage({ viewport: { width: 1400, height: 1000 }, deviceScaleFactor: 1 });
  const errors = [];
  page.on('console', message => errors.push(`console ${message.type()}: ${message.text()}`));
  page.on('pageerror', error => errors.push(`pageerror: ${error.message}`));
  page.on('requestfailed', request => errors.push(`requestfailed: ${request.url()} ${request.failure()?.errorText}`));
  await page.goto(`file://${pdf}`, { waitUntil: 'load', timeout: 30000 });
  await page.waitForTimeout(2500);
  const pdfViewer = await page.evaluate(() => {
    const embed = document.querySelector('embed');
    return {
      title: document.title,
      url: location.href,
      embedType: embed?.type ?? null,
      bodyBytes: document.body?.innerHTML?.length ?? 0,
      embedBytes: embed?.outerHTML?.length ?? 0,
    };
  });
  await page.screenshot({ path: out, fullPage: false });
  console.log(JSON.stringify({ pdfViewer, errors }, null, 2));
  await browser.close();
})().catch(error => {
  console.error(error);
  process.exit(1);
});
