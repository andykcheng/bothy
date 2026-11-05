#!/usr/bin/env node
// Checks Tinkerworks stages (app/assets/games/tinkerworks): each stage must fail with no parts and have a
// solution that still works when its parts are nudged a few pixels. Searches for one when `sol` is missing
// or broken (random restarts + hill climbing on how close the target gets to the goal).
//   node scripts/tinker_check.js [stage numbers, 1-based ...] [--secs N] [--json]
'use strict';
const path = require('path');
const dir = path.join(__dirname, '../app/assets/games/tinkerworks');
const Tinker = require(path.join(dir, 'sim.js')), LEVELS = require(path.join(dir, 'levels.js'));
const { PARTS, W, H, fits, simulate } = Tinker;
const args = process.argv.slice(2), secs = +(args[args.indexOf('--secs') + 1] || 60) || 60, asJson = args.includes('--json');
const which = args.filter((a, i) => /^\d+$/.test(a) && args[i - 1] !== '--secs').map(Number);
const R = (a, b) => a + Math.random() * (b - a), pick = a => a[Math.floor(Math.random() * a.length)];

function randomPart(t, near) {
  const p = { t, x: 0, y: 0, a: 0 };
  if (near && Math.random() < .7) { const q = pick(near); p.x = q.x + R(-50, 50); p.y = q.y + R(-20, 70); } else { p.x = R(15, W - 15); p.y = R(20, H - 15); }
  p.x = Math.round(p.x / 5) * 5; p.y = Math.round(p.y / 5) * 5;
  const turn = PARTS[t].turn;
  if (turn === 90) p.a = pick([0, 90, 180, 270]); else if (turn) p.a = t === 'seesaw' ? pick([-30, -15, 0, 15, 30]) : turn * Math.round(R(-75, 75) / turn);
  if (PARTS[t].flip) p.f = pick([1, -1]);
  return p;
}
function trail(level) {   // where things move with no parts: good places to try parts
  const w = Tinker.build(level, []), pts = [];
  for (let t = 0; t < 600; t++) { Tinker.step(w); if (t % 6 === 0) for (const b of w.bodies) if (!b.isStatic && b.speed > .3) pts.push({ x: b.position.x, y: b.position.y }); }
  return pts.length ? pts : null;
}
const valid = (level, placed) => placed.every((_, i) => fits(level, placed, i));
function robust(level, placed, n = 12) {   // fraction of nudged copies (±4 px, each part) that still win
  let ok = 0;
  for (let k = 0; k < n; k++) {
    const q = placed.map(p => ({ ...p, x: p.x + Math.round(R(-4, 4)), y: p.y + Math.round(R(-4, 4)) }));
    if (!valid(level, q)) { k--; continue; }
    if (simulate(level, q).win) ok++;
  }
  return ok / n;
}
function solve(level, budgetMs) {
  const t0 = Date.now(), list = Object.entries(level.parts).flatMap(([t, n]) => Array(n).fill(t)), near = trail(level);
  let best = null;
  while (Date.now() - t0 < budgetMs) {
    // random restart
    let cur = null, cs = 1e9;
    for (let k = 0; k < 40 && !cur; k++) { const c = list.map(t => randomPart(t, near)); if (valid(level, c)) cur = c; }
    if (!cur) continue;
    cs = simulate(level, cur).score;
    for (let it = 0; it < 60 && Date.now() - t0 < budgetMs; it++) {
      const nx = cur.map(p => ({ ...p })), i = Math.floor(Math.random() * nx.length), p = nx[i];
      if (Math.random() < .25) nx[i] = randomPart(p.t, near);
      else { p.x += Math.round(R(-25, 25) / 5) * 5; p.y += Math.round(R(-25, 25) / 5) * 5; const turn = PARTS[p.t].turn; if (turn && turn < 90 && Math.random() < .4) p.a += pick([-turn, turn]); if (PARTS[p.t].flip && Math.random() < .1) p.f = -p.f; }
      if (!valid(level, nx)) continue;
      const s = simulate(level, nx).score;
      if (s <= cs) { cur = nx; cs = s; }
      if (cs < 0) break;
    }
    if (cs < 0) { const r = robust(level, cur); if (!best || r > best.r) best = { sol: cur, r }; if (best.r >= .75) break; }
  }
  return best;
}
const out = [];
for (const [i, level] of LEVELS.entries()) {
  if (which.length && !which.includes(i + 1)) continue;
  const t0 = Date.now(), res = { stage: i + 1, n: level.n, emptyWins: simulate(level, []).win };
  if (level.sol && valid(level, level.sol) && simulate(level, level.sol).win) { res.sol = level.sol; res.r = robust(level, level.sol); res.given = true; }
  if (!res.sol || res.r < .5) { const b = solve(level, secs * 1000); if (b && (!res.sol || b.r > res.r)) { res.sol = b.sol; res.r = b.r; res.given = false; } }
  res.secs = Math.round((Date.now() - t0) / 1000);
  out.push(res);
  if (!asJson) console.log(`${String(i + 1).padStart(2)} ${level.n.padEnd(16)} ${res.emptyWins ? 'SOLVED WITH NO PARTS ' : ''}${res.sol ? `ok, robust ${Math.round(res.r * 100)}%${res.given ? '' : ' (found)'} sol: ${JSON.stringify(res.sol)}` : 'NO SOLUTION FOUND'} [${res.secs}s]`);
}
if (asJson) console.log(JSON.stringify(out));
process.exitCode = out.every(r => r.sol && !r.emptyWins && r.r >= .5) ? 0 : 1;
