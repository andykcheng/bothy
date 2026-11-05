// Tinkerworks physics: builds a stage (fixed objects + the player's parts) as a matter.js world and runs it.
// Shared by the game page and by the offline level checker (node), so both see identical physics.
(function (root) {
  'use strict';
  const Matter = root.Matter || require('./matter.min.js');
  const { Engine, Bodies, Body, Composite, Events, Constraint, Query } = Matter;
  const W = 360, H = 540, STEP = 1000 / 60;

  // what the player can place: size, whether it turns, whether it flips
  const PARTS = {
    plank: { n: 'Plank', turn: 15 }, short: { n: 'Short plank', turn: 15 }, bouncer: { n: 'Bouncer', turn: 15 },
    fan: { n: 'Fan', turn: 90 }, conveyor: { n: 'Conveyor', flip: 1 }, seesaw: { n: 'Seesaw', turn: 15 },
    ball: { n: 'Ball', turn: 0 }, heavy: { n: 'Iron ball', turn: 0 }, crate: { n: 'Crate', turn: 0 }, domino: { n: 'Domino', turn: 0 },
    balloon: { n: 'Balloon', turn: 0 }, pin: { n: 'Peg', turn: 0 },
  };
  const deg = d => d * Math.PI / 180;

  // one object (fixed by the stage, or a placed part) -> bodies (+ constraints)
  function make(o, part) {
    const a = deg(o.a || 0), st = { isStatic: true, angle: a, friction: .3 }, out = [], cons = [];
    // (matter.js zeroes restitution and maxes friction on static bodies: put ours back)
    const tag = (b, kind, extra = {}) => { b.plugin = { kind, part: !!part, ...extra }; if (b.isStatic) { b.restitution = kind === 'bouncer' ? 1.15 : kind === 'pin' ? .5 : .1; b.friction = kind === 'bouncer' ? 0 : kind === 'conveyor' ? 1 : .5; } out.push(b); return b; };
    switch (o.t) {
      case 'wall': tag(Bodies.rectangle(o.x, o.y, o.w, o.h, { ...st }), 'wall'); break;
      case 'plank': tag(Bodies.rectangle(o.x, o.y, 110, 10, { ...st, friction: .2 }), 'plank'); break;
      case 'short': tag(Bodies.rectangle(o.x, o.y, 60, 10, { ...st, friction: .2 }), 'plank'); break;
      case 'bouncer': tag(Bodies.rectangle(o.x, o.y, 60, 12, { ...st, restitution: 1.15, friction: 0 }), 'bouncer'); break;
      case 'pin': tag(Bodies.circle(o.x, o.y, 6, { ...st, restitution: .5 }), 'pin'); break;
      case 'fan': tag(Bodies.rectangle(o.x, o.y, 26, 26, { ...st }), 'fan', { dir: a }); break;
      case 'conveyor': tag(Bodies.rectangle(o.x, o.y, 110, 12, { ...st, angle: 0, friction: 1 }), 'conveyor', { dir: o.f === -1 ? -1 : 1 }); break;
      case 'ball': tag(Bodies.circle(o.x, o.y, 12, { restitution: .35, friction: .1, frictionAir: .002, density: .002 }), o.target ? 'target' : 'ball'); break;
      case 'heavy': tag(Bodies.circle(o.x, o.y, 17, { restitution: .15, friction: .1, frictionAir: .001, density: .008 }), o.target ? 'target' : 'heavy'); break;
      case 'crate': tag(Bodies.rectangle(o.x, o.y, 34, 34, { angle: a, friction: .5, density: o.target ? .001 : .002 }), o.target ? 'target' : 'crate'); break;
      case 'domino': tag(Bodies.rectangle(o.x, o.y, 8, 54, { angle: a, friction: .1, density: .001, chamfer: { radius: 3 } }), 'domino'); break;
      case 'balloon': tag(Bodies.circle(o.x, o.y, 16, { restitution: .3, friction: .05, frictionAir: .025, density: .0008 }), o.target ? 'target' : 'balloon', { balloon: 1 }); break;
      case 'seesaw': {
        const b = tag(Bodies.rectangle(o.x, o.y, 140, 8, { angle: a, friction: .4, density: .0015 }), 'seesaw');
        cons.push(Constraint.create({ pointA: { x: o.x, y: o.y }, bodyB: b, pointB: { x: 0, y: 0 }, stiffness: 1, length: 0 }));
        break;
      }
      case 'basket': {   // open-topped box; (x, y) is the middle of its floor
        tag(Bodies.rectangle(o.x, o.y - 4, 70, 8, st), 'basket'); tag(Bodies.rectangle(o.x - 31, o.y - 26, 8, 48, st), 'basket'); tag(Bodies.rectangle(o.x + 31, o.y - 26, 8, 48, st), 'basket');
        break;
      }
      case 'button': tag(Bodies.rectangle(o.x, o.y, 44, 10, { ...st }), 'button', { gate: o.gate }); break;
      case 'gate': tag(Bodies.rectangle(o.x, o.y, o.w, o.h, { ...st }), 'gate', { id: o.id }); break;
    }
    if (o.target) out.forEach(b => { b.plugin.target = 1; b.plugin.balloon = o.t === 'balloon' ? 1 : 0; });
    return { bodies: out, cons };
  }
  function goalOf(level) {
    if (level.goal) return level.goal;
    const b = level.fixed.find(o => o.t === 'basket' && o.goal);
    return { x: b.x - 26, y: b.y - 46, w: 52, h: 40 };
  }
  function build(level, placed) {
    const engine = Engine.create({ positionIterations: 10, velocityIterations: 8 });
    engine.gravity.y = 1;
    const all = [], cons = [];
    const add = (o, part) => { const r = make(o, part); all.push(...r.bodies); cons.push(...r.cons); return r; };
    // the frame: floor, sides (the top is open)
    add({ t: 'wall', x: W / 2, y: H + 20, w: W + 80, h: 40 }); add({ t: 'wall', x: -20, y: H / 2 - 200, w: 40, h: H + 400 }); add({ t: 'wall', x: W + 20, y: H / 2 - 200, w: 40, h: H + 400 });
    all.forEach(b => b.plugin.frame = 1);
    for (const o of level.fixed) add(o, false);
    const partBodies = placed.map(p => add(p, true).bodies);
    Composite.add(engine.world, [...all, ...cons]);
    const w = { engine, bodies: all, partBodies, goal: goalOf(level), target: all.find(b => b.plugin.target), t: 0, hold: 0, won: false, pressed: {}, wind: all.filter(b => b.plugin.kind === 'fan') };
    Events.on(engine, 'collisionStart', e => {
      for (const p of e.pairs) for (const [a, b] of [[p.bodyA, p.bodyB], [p.bodyB, p.bodyA]]) {
        if (a.plugin.kind === 'button' && !b.isStatic && !w.pressed[a.plugin.gate]) {
          w.pressed[a.plugin.gate] = 1; a.plugin.on = 1;
          for (const g of all.filter(g => g.plugin.kind === 'gate' && g.plugin.id === a.plugin.gate)) { Composite.remove(engine.world, g); g.plugin.gone = 1; }
        }
      }
    });
    const belt = e => {
      for (const p of e.pairs) for (const [a, b] of [[p.bodyA, p.bodyB], [p.bodyB, p.bodyA]]) {
        if (a.plugin.kind === 'conveyor' && !b.isStatic && b.position.y < a.position.y) { const v = 2.6 * a.plugin.dir; Body.setVelocity(b, { x: b.velocity.x + (v - b.velocity.x) * .35, y: b.velocity.y }); }
      }
    };
    Events.on(engine, 'collisionActive', belt); Events.on(engine, 'collisionStart', belt);
    return w;
  }
  function step(w) {
    const g = w.engine.gravity;
    for (const b of w.bodies) {
      if (b.isStatic) continue;
      if (b.plugin.balloon) Body.applyForce(b, b.position, { x: 0, y: -b.mass * g.y * g.scale * 2 });
      for (const f of w.wind) {   // fans push along their facing, within a 44-wide, 170-long stream
        const dx = b.position.x - f.position.x, dy = b.position.y - f.position.y, c = Math.cos(f.plugin.dir), s = Math.sin(f.plugin.dir);
        const along = dx * c + dy * s, across = -dx * s + dy * c;
        if (along > 10 && along < 180 && Math.abs(across) < 24) { const k = 3.2e-6 * b.area * (1 - along / 200); Body.applyForce(b, b.position, { x: c * k, y: s * k }); }
      }
    }
    Engine.update(w.engine, STEP); w.t++;
    const p = w.target.position, G = w.goal;
    if (p.x > G.x && p.x < G.x + G.w && p.y > G.y && p.y < G.y + G.h) { if (++w.hold >= 12) w.won = true; } else w.hold = 0;
    return w.won;
  }
  // can parts[i] go where it is? (inside the frame, not overlapping anything)
  function fits(level, placed, i) {
    const w = build(level, placed), mine = w.partBodies[i];
    const others = w.bodies.filter(b => !mine.includes(b));
    for (const b of mine) {
      if (b.bounds.min.x < 0 || b.bounds.max.x > W || b.bounds.max.y > H || b.bounds.min.y < 0) return false;
      for (const z of level.nobuild || []) if (b.bounds.max.x > z.x && b.bounds.min.x < z.x + z.w && b.bounds.max.y > z.y && b.bounds.min.y < z.y + z.h) return false;
      if (Query.collides(b, others).length) return false;
    }
    return true;
  }
  // run a stage to the end; score = how close the target got to the goal (lower is better; wins are negative)
  function simulate(level, placed, max = 900) {
    const w = build(level, placed), G = w.goal, gx = G.x + G.w / 2, gy = G.y + G.h / 2; let best = 1e9;
    while (w.t < max) {
      if (step(w)) return { win: true, t: w.t, score: -1000 + w.t / 100 };
      const p = w.target.position; best = Math.min(best, Math.hypot(p.x - gx, p.y - gy));
    }
    return { win: false, t: w.t, score: best - 150 * Object.keys(w.pressed).length };
  }
  const api = { W, H, PARTS, build, step, fits, simulate, goalOf, deg };
  if (typeof module !== 'undefined') module.exports = api; else root.Tinker = api;
})(typeof window !== 'undefined' ? window : globalThis);
