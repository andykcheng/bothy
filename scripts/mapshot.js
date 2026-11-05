// Render an sp-server map page in headless Chromium (software WebGL) and screenshot it.
// node scripts/mapshot.js <playwright-core dir> <url> <out.png>
// e.g. playwright-core from the npx cache: ls ~/.npm/_npx/*/node_modules/playwright-core
const [pw, url, out] = process.argv.slice(2);
const { chromium } = require(pw);
(async () => {
  const exe = require('os').homedir() + '/.cache/ms-playwright/chromium-1248/chrome-linux-arm64/chrome';
  const b = await chromium.launch({ executablePath: exe, args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader'] });
  const p = await b.newPage({ viewport: { width: 1000, height: 700 } });
  p.on('console', m => console.log('console', m.type(), m.text()));
  p.on('pageerror', e => console.log('pageerror', e.message));
  let tiles = 0, bad = [];
  p.on('response', r => { if (r.url().includes('/tiles/')) tiles++; if (r.status() >= 400) bad.push(r.status() + ' ' + r.url()); });
  await p.goto(url);
  await p.waitForTimeout(12000);
  await p.screenshot({ path: out });
  console.log(JSON.stringify({ tiles, bad: bad.slice(0, 5) }));
  await b.close();
})().catch(e => { console.error(e); process.exit(1); });
