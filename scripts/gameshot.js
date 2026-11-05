// Cover images for the games grid: open each game in headless Chromium and screenshot its first screen.
// node scripts/gameshot.js <playwright-core dir> <base url of a server over the games dir> <games dir> [id ...]
// Writes <games dir>/<id>/cover.jpg (600x450, 4:3) unless the game already has a hand-made cover.png/cover.jpg
// listed in game.json with "cover_manual": true. Adds "cover": "cover.jpg" to game.json.
// "cover_click": [button texts] are clicked first (e.g. to start a game so the cover shows the board).
const [pw, base, dir, ...only] = process.argv.slice(2);
const fs = require('fs'), path = require('path');
const { chromium } = require(pw);
(async () => {
  const exe = require('os').homedir() + '/.cache/ms-playwright/chromium-1248/chrome-linux-arm64/chrome';
  const b = await chromium.launch({ executablePath: exe, args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader'] });
  const ids = only.length ? only : fs.readdirSync(dir).filter(d => fs.existsSync(path.join(dir, d, 'game.json')));
  for (const id of ids) {
    const metaPath = path.join(dir, id, 'game.json');
    const meta = JSON.parse(fs.readFileSync(metaPath, 'utf8'));
    if (meta.cover_manual) { console.log(id, 'manual cover, skipped'); continue; }
    const ctx = await b.newContext({ viewport: { width: 600, height: 450 }, deviceScaleFactor: 1, colorScheme: 'light' });
    const p = await ctx.newPage();
    const errs = [];
    p.on('pageerror', e => errs.push(e.message));
    await p.goto(new URL(id + '/', base).href, { waitUntil: 'load' });
    await p.waitForTimeout(meta.cover_wait_ms || 2500);
    // "cover_click": button texts to press first, e.g. to start a game so the cover shows the board
    for (const t of meta.cover_click || []) {
      try { await p.getByText(t, { exact: true }).first().click({ timeout: 3000 }); await p.waitForTimeout(1200); }
      catch (e) { console.log(id, 'could not click', JSON.stringify(t)); }
    }
    // the same paper the app's game screen gives pages with a transparent background
    await p.evaluate(() => {
      const clear = e => { const s = getComputedStyle(e), c = s.backgroundColor; return (c === 'transparent' || c === 'rgba(0, 0, 0, 0)') && s.backgroundImage === 'none'; };
      if (document.body && clear(document.documentElement) && clear(document.body)) document.documentElement.style.background = '#fff';
    });
    await p.screenshot({ path: path.join(dir, id, 'cover.jpg'), type: 'jpeg', quality: 78 });
    meta.cover = 'cover.jpg';
    fs.writeFileSync(metaPath, JSON.stringify(meta, null, 2) + '\n');
    console.log(id, fs.statSync(path.join(dir, id, 'cover.jpg')).size, errs.length ? 'errors: ' + errs.slice(0, 2).join(' | ') : '');
    await ctx.close();
  }
  await b.close();
})().catch(e => { console.error(e); process.exit(1); });
