/* Patterns and mirror (M6.6, M6.7): the plate of docs/design/m6.0-patterns.md section 2.2 as a
   second part, its stand-in core (each pattern's instances and their verdicts, as the `pattern`
   read gives them), the Rectangular Pattern, Circular Pattern, and Mirror dialogs (section 7),
   and the instances in the canvas: Suppression's boxes, the marks, and the flip arrows. */
(function () {
  const A = () => window.APP;
  const T = THREE;
  const S = () => A().S;
  const esc = (s) => A().esc(s);
  const clone = (o) => JSON.parse(JSON.stringify(o));
  // Each type's command in the registry (app.js's CMD): its label is the dialog's title, its icon the type's.
  const CMD_OF = { linear_pattern: 'rectPattern', polar_pattern: 'circPattern', mirror: 'mirror' };
  const TYPES = new Proxy({}, { get: (o, t) => A().FEATURE_KIND[t] });
  const TITLE = new Proxy({}, { get: (o, t) => A().CMD[CMD_OF[t]].label });
  const ICON = new Proxy({}, { get: (o, t) => A().CMD[CMD_OF[t]].icon });
  const isPattern = (t) => t in CMD_OF;
  const SWEEPS = ['pad', 'pocket', 'revolve', 'groove'];

  // ───────── The plate, as section 2.2 writes it (its chamfer left out of the stand-in) ─────────
  function plateFeatures() {
    return [
      { id: 's0000001', type: 'sketch', name: 'Plate outline', plane: 'XY', inputs: [] },
      { id: 'p0000001', type: 'pad', name: 'Plate', inputs: ['s0000001'] },
      { id: 's0000002', type: 'sketch', name: 'Notch outline', plane: 'XY', inputs: [] },
      { id: 'q0000001', type: 'pocket', name: 'Notch', inputs: ['p0000001', 's0000002'] },
      { id: 'm0000001', type: 'mirror', name: 'Other notch', inputs: ['q0000001'] },
      { id: 's0000003', type: 'sketch', name: 'Corner hole outline', plane: 'XY', inputs: [] },
      { id: 'q0000002', type: 'pocket', name: 'Corner hole', inputs: ['m0000001', 's0000003'] },
      { id: 't0000001', type: 'linear_pattern', name: 'Corner holes', inputs: ['q0000002'] },
      { id: 's0000004', type: 'sketch', name: 'Boss outline', plane: 'Face', inputs: ['p0000001'] },
      { id: 'p0000002', type: 'pad', name: 'Boss', inputs: ['t0000001', 's0000004'] },
      { id: 's0000005', type: 'sketch', name: 'Boss hole outline', plane: 'XY', inputs: [] },
      { id: 'q0000003', type: 'pocket', name: 'Boss hole', inputs: ['p0000002', 's0000005'] },
      { id: 't0000002', type: 'polar_pattern', name: 'Boss holes', inputs: ['q0000003'] },
    ];
  }
  function platePatterns() {
    return {
      m0000001: { copies: { type: 'features', features: ['q0000001'] }, plane: { word: 'yz' } },
      t0000001: { copies: { type: 'features', features: ['q0000002'] }, skip: [],
        directions: [{ along: { word: 'x' }, reversed: false, dist: 'extent', length: 80, count: 3 }, { along: { word: 'y' }, reversed: false, dist: 'extent', length: 40, count: 2 }] },
      t0000002: { copies: { type: 'features', features: ['q0000003'] }, skip: [], axis: { word: 'z' }, reversed: false, dist: 'extent', angle: 360, count: 4 },
    };
  }
  // What each sweep's tool is, and where its original sits (the `pattern` read's anchor).
  const TOOLS = {
    q0000001: { kind: 'rect', x0: 44, x1: 50, y0: -5, y1: 5, cut: true, anchor: [47, 0, 0], area: 60,
      names: { x0: 'Edge.nb000004', x1: 'Edge.nb000002', y0: 'Edge.nb000001', y1: 'Edge.nb000003' }, inner: (n) => `q0000001.pocket.Face(G s0000002.sketch.${n})` },
    q0000002: { kind: 'circle', cx: -40, cy: -20, r: 2.75, cut: true, anchor: [-40, -20, 0], area: Math.PI * 2.75 * 2.75, inner: () => 'q0000002.pocket.Face(G s0000003.sketch.Edge.cc000001)' },
    p0000002: { kind: 'boss', cx: 0, cy: 0, r: 8, z0: 5, z1: 15, cut: false, anchor: [0, 0, 5], area: Math.PI * 64,
      inner: (n) => n === 'top' ? 'p0000002.pad.Face(G s0000004.sketch.Face1)' : 'p0000002.pad.Face(G s0000004.sketch.Edge.cd000001)' },
    q0000003: { kind: 'circle', cx: 5, cy: 0, r: 1, cut: true, anchor: [5, 0, 0], area: Math.PI, inner: () => 'q0000003.pocket.Face(G s0000005.sketch.Edge.ce000001)' },
  };
  const PLATE_SKETCH_ROWS = {
    s0000001: ['XY', '4 lines', 'None'], s0000002: ['XY', '4 lines', 'None'], s0000003: ['XY', '1 circle, ⌀ 5.5 mm', 'None'],
    s0000004: ['Face', '1 circle, ⌀ 16 mm', 'None'], s0000005: ['XY', '1 circle, ⌀ 2 mm', 'None'],
  };
  // Who goes with a feature when it's suppressed: its true children (Fable's rule), and a
  // pattern is a child of each original it copies (m6.0 section 6).
  function children(d) {
    const ids = d.features.map((f) => f.id);
    const after = (id) => ids.slice(ids.indexOf(id) + 1);
    const ch = {
      s0000001: after('s0000001').filter((x) => !['s0000002', 's0000003', 's0000005'].includes(x)),
      p0000001: after('p0000001').filter((x) => !['s0000002', 's0000003', 's0000005'].includes(x)),
      s0000002: ['q0000001'], s0000003: ['q0000002'], s0000004: ['p0000002'], s0000005: ['q0000003'],
    };
    for (const f of d.features) if (isPattern(f.type) && d.pat[f.id] && d.pat[f.id].copies.type === 'features') {
      for (const o of d.pat[f.id].copies.features) (ch[o] = ch[o] || []).push(f.id);
    }
    // Closed over: a child's children go too.
    const out = {};
    for (const id of ids) { const seen = new Set(); const st = [...(ch[id] || [])]; while (st.length) { const x = st.pop(); if (seen.has(x)) continue; seen.add(x); st.push(...(ch[x] || [])); } out[id] = [...seen]; }
    return out;
  }

  // ───────── References: an origin word, or an element of the model by its stable name ─────────
  const WORDS = { x: [1, 0, 0], y: [0, 1, 0], z: [0, 0, 1] };
  const PLANES = { xy: { origin: [0, 0, 0], normal: [0, 0, 1] }, xz: { origin: [0, 0, 0], normal: [0, 1, 0] }, yz: { origin: [0, 0, 0], normal: [1, 0, 0] } };
  function refLabel(r) {
    if (!r) return '';
    if (r.word) return r.word.length === 1 ? r.word.toUpperCase() + ' axis' : r.word.toUpperCase() + ' plane';
    return r.label;
  }
  function dirOf(r, rev) { const v = r.word ? WORDS[r.word] : r.vec; return v.map((c) => (rev ? -c : c)); }
  const shortName = (n) => (n.length > 44 ? n.slice(0, 42) + '…' : n);

  // ───────── Instances: index, placement, anchor ─────────
  const R = (v) => Math.round(v * 1e6) / 1e6 + 0;
  function instancesOf(type, P) {
    const out = [];
    if (type === 'mirror') { out.push({ index: [1], place: { kind: 'mirror', plane: P.plane } }); return out; }
    if (type === 'linear_pattern') {
      const dirs = P.directions.filter((x, i) => i === 0 || x.count > 1);
      const step = (x) => (x.count > 1 ? (x.dist === 'extent' ? x.length / (x.count - 1) : x.length) : 0);
      const c1 = dirs[0].count, c2 = dirs[1] ? dirs[1].count : 1;
      for (let i = 0; i < c1; i++) for (let j = 0; j < c2; j++) {
        if (!i && !j) continue;
        const u1 = dirOf(dirs[0].along, dirs[0].reversed), u2 = dirs[1] ? dirOf(dirs[1].along, dirs[1].reversed) : [0, 0, 0];
        const s1 = i * step(dirs[0]), s2 = j * (dirs[1] ? step(dirs[1]) : 0);
        const v = [0, 1, 2].map((k) => R(u1[k] * s1 + u2[k] * s2));
        out.push({ index: j ? [i, j] : [i], place: { kind: 'move', v } });
      }
      return out;
    }
    const step = P.dist === 'spacing' ? P.angle : P.count > 1 ? (P.angle === 360 ? P.angle / P.count : P.angle / (P.count - 1)) : 0;
    const ax = P.axis.word ? { point: [0, 0, 0], dir: WORDS[P.axis.word] } : { point: P.axis.point, dir: P.axis.vec };
    for (let k = 1; k < P.count; k++) out.push({ index: [k], place: { kind: 'turn', point: ax.point, dir: ax.dir, angle: R(k * step * (P.reversed ? -1 : 1)) } });
    return out;
  }
  const idxText = (ix) => ix.join(',');
  const idxCmp = (a, b) => (a[0] - b[0]) || ((a[1] || 0) - (b[1] || 0));
  const skipped = (P, ix) => (P.skip || []).some((s) => idxText(s) === idxText(ix));
  function placePoint(p, pl) {
    if (pl.kind === 'move') return [p[0] + pl.v[0], p[1] + pl.v[1], p[2] + pl.v[2]];
    if (pl.kind === 'mirror') {
      const o = pl.plane.word ? PLANES[pl.plane.word].origin : pl.plane.origin, n = pl.plane.word ? PLANES[pl.plane.word].normal : pl.plane.normal;
      const dd = (p[0] - o[0]) * n[0] + (p[1] - o[1]) * n[1] + (p[2] - o[2]) * n[2];
      return [0, 1, 2].map((k) => p[k] - 2 * dd * n[k]);
    }
    const v = new T.Vector3(...p).sub(new T.Vector3(...pl.point)).applyAxisAngle(new T.Vector3(...pl.dir).normalize(), pl.angle * Math.PI / 180).add(new T.Vector3(...pl.point));
    return [v.x, v.y, v.z];
  }
  // A tool placed, or null where the stand-in can't draw it (a turn about an axis that isn't vertical,
  // a notch turned off the grid).
  function placeTool(t, pl) {
    const near = (a, b) => Math.abs(a - b) < 1e-6;
    if (pl.kind === 'turn' && !(near(Math.abs(pl.dir[2]), 1))) return null;
    const P = (x, y, z) => placePoint([x, y, z || 0], pl);
    if (t.kind === 'circle' || t.kind === 'boss') {
      const c = P(t.cx, t.cy, t.kind === 'boss' ? t.z0 : 0); const out = Object.assign({}, t, { cx: R(c[0]), cy: R(c[1]) });
      if (t.kind === 'boss') { const c1 = P(t.cx, t.cy, t.z1); out.z0 = R(Math.min(c[2], c1[2])); out.z1 = R(Math.max(c[2], c1[2])); }
      else out.dz = R(c[2]);
      return out;
    }
    // A notch: each side's midpoint placed, the box rebuilt, each side keeping its name.
    const mids = { x0: [t.x0, (t.y0 + t.y1) / 2], x1: [t.x1, (t.y0 + t.y1) / 2], y0: [(t.x0 + t.x1) / 2, t.y0], y1: [(t.x0 + t.x1) / 2, t.y1] };
    const pts = [[t.x0, t.y0], [t.x1, t.y1]].map(([x, y]) => P(x, y));
    const box = { x0: R(Math.min(pts[0][0], pts[1][0])), x1: R(Math.max(pts[0][0], pts[1][0])), y0: R(Math.min(pts[0][1], pts[1][1])), y1: R(Math.max(pts[0][1], pts[1][1])) };
    const names = {};
    for (const [side, m] of Object.entries(mids)) {
      const q = P(m[0], m[1]); const to = near(q[0], box.x0) ? 'x0' : near(q[0], box.x1) ? 'x1' : near(q[1], box.y0) ? 'y0' : near(q[1], box.y1) ? 'y1' : null;
      if (!to) return null;
      names[to] = t.names[side];
    }
    return Object.assign({}, t, box, { names, dz: R(P(0, 0, 0)[2]) });
  }

  // ───────── The stand-in core: each step in timeline order, each copy's verdict ─────────
  function evaluate(d, health, problems) {
    const ok = (id) => health[id] === 'healthy';
    const geo = { rects: [], circles: [], bosses: [] };
    const instances = {};
    if (!ok('p0000001')) return { geo: null, instances };
    const plate = (x, y) => x > -50 && x < 50 && y > -30 && y < 30;
    const filled = (x, y) => plate(x, y) && !geo.rects.some((r) => x > r.x0 && x < r.x1 && y > r.y0 && y < r.y1);
    const ring = (cx, cy, r) => { const p = [[cx, cy]]; for (let i = 0; i < 16; i++) { const t = i / 16 * Math.PI * 2; p.push([cx + Math.cos(t) * r * 0.98, cy + Math.sin(t) * r * 0.98]); } return p; };
    const nameOf = (stableInner, f, ix) => f ? `${f.id}.${f.type}.Face(C${idxText(ix)} ${stableInner})` : stableInner;
    // Apply one tool; say whether it changed the solid ('built'), changed nothing, or lies apart from it.
    function apply(t, maker, f, ix) {
      if (t.kind === 'circle') {
        if (t.dz && Math.abs(t.dz) > 1e-9) return 'changed_nothing'; // a through-all cut moved along its own sweep lands on itself
        const inside = ring(t.cx, t.cy, t.r).some(([x, y]) => filled(x, y));
        const dup = geo.circles.some((c) => Math.hypot(c.cx - t.cx, c.cy - t.cy) + t.r <= c.r + 1e-9);
        if (!inside || dup) return 'changed_nothing';
        geo.circles.push({ cx: t.cx, cy: t.cy, r: t.r, maker, stable: nameOf(t.inner(), f, ix) });
        return 'built';
      }
      if (t.kind === 'rect') {
        if (t.dz && Math.abs(t.dz) > 1e-9) return 'changed_nothing';
        let any = false;
        for (let i = 0; i <= 6 && !any; i++) for (let j = 0; j <= 6 && !any; j++) any = filled(t.x0 + (t.x1 - t.x0) * (i + 0.5) / 7.5, t.y0 + (t.y1 - t.y0) * (j + 0.5) / 7.5);
        if (!any) return 'changed_nothing';
        const names = {}; for (const k of ['x0', 'x1', 'y0', 'y1']) names[k] = nameOf(t.inner(t.names[k]), f, ix);
        geo.rects.push({ x0: t.x0, x1: t.x1, y0: t.y0, y1: t.y1, maker, names });
        return 'built';
      }
      // A boss: fused on the plate's top face, or on another boss's top.
      const touchesPlate = Math.abs(t.z0 - 5) < 1e-9 && ring(t.cx, t.cy, t.r).some(([x, y]) => plate(x, y));
      const onBoss = geo.bosses.some((b) => Math.abs(t.z0 - b.z1) < 1e-9 && Math.hypot(b.cx - t.cx, b.cy - t.cy) < b.r + t.r);
      const within = geo.bosses.some((b) => Math.hypot(b.cx - t.cx, b.cy - t.cy) + t.r <= b.r + 1e-9 && t.z0 >= b.z0 - 1e-9 && t.z1 <= b.z1 + 1e-9) || (t.z1 <= 5 + 1e-9 && t.z0 >= 0 - 1e-9);
      if (within) return 'changed_nothing';
      if (!touchesPlate && !onBoss) return 'off_base';
      geo.bosses.push({ cx: t.cx, cy: t.cy, r: t.r, z0: t.z0, z1: t.z1, maker, side: nameOf(t.inner('side'), f, ix), top: nameOf(t.inner('top'), f, ix) });
      return 'built';
    }
    for (const f of d.features) {
      if (!ok(f.id)) continue;
      if (TOOLS[f.id]) { apply(TOOLS[f.id], f.id, null, null); continue; }
      if (!isPattern(f.type)) continue;
      const P = d.pat[f.id]; if (!P) continue;
      // Bodies: the stand-in draws no whole-body copies, so they have no verdict (state null), each
      // anchored at the base's centre of mass, as the read anchors a solid's copies (m6.0 section 5).
      if (P.copies.type !== 'features') {
        const anchor = [0, 0, 2.5];
        instances[f.id] = { bodies: true, anchor, list: instancesOf(f.type, P).map((x) => ({ ...x, anchor: placePoint(anchor, x.place), state: skipped(P, x.index) ? 'skipped' : null })).sort((a, b) => idxCmp(a.index, b.index)) };
        continue;
      }
      // Two directions along one line: the gate's refusal (m6.0 section 4.2), before anything is placed.
      if (f.type === 'linear_pattern' && P.directions.length > 1) {
        const a = dirOf(P.directions[0].along), b = dirOf(P.directions[1].along);
        if (Math.abs(Math.abs(a[0] * b[0] + a[1] * b[1] + a[2] * b[2]) - 1) < 1e-12) {
          health[f.id] = 'error'; instances[f.id] = { list: [], anchor: null };
          const word = (r) => (r.word ? r.word : `"${r.name.slice(0, 40)}"…`);
          problems.push({ feature: f.id, severity: 'error', code: 'invalid_input', title: `${f.name}'s directions are parallel`,
            message: `linear_pattern ${f.id}'s directions are parallel: directions.1.along, ${word(P.directions[1].along)}, runs along directions.0.along, ${word(P.directions[0].along)}, so copies would land on copies`,
            elements: P.directions[1].along.name ? [P.directions[1].along.name] : [], param: 'directions.1.along', fix: 'give directions.1 another way, or drop it and raise directions.0.count' });
          continue;
        }
      }
      const originals = P.copies.features.filter((o) => TOOLS[o]).sort((a, b) => d.features.findIndex((x) => x.id === a) - d.features.findIndex((x) => x.id === b));
      const list = instancesOf(f.type, P).map((x) => ({ ...x, state: skipped(P, x.index) ? 'skipped' : 'built', verdicts: [] }));
      // Several originals: Σ(areaᵢ · centreᵢ) / Σ areaᵢ over their profiles (m6.0 section 12, decided 6).
      const total = originals.reduce((s, o) => s + TOOLS[o].area, 0);
      const anchor = [0, 1, 2].map((k) => originals.reduce((s, o) => s + TOOLS[o].area * TOOLS[o].anchor[k], 0) / Math.max(1e-12, total));
      const before = clone(geo);
      for (const o of originals) {
        const t = TOOLS[o];
        for (const x of list) {
          if (x.state === 'skipped') continue;
          const placed = placeTool(t, x.place);
          if (!placed) { x.verdicts.push(null); continue; }
          x.verdicts.push(apply(placed, f.id, f, x.index));
        }
      }
      for (const x of list) {
        x.anchor = placePoint(anchor, x.place);
        if (x.state === 'skipped') continue;
        x.state = x.verdicts.includes('off_base') ? 'off_base' : x.verdicts.includes('changed_nothing') ? 'changed_nothing' : x.verdicts.includes(null) ? null : 'built';
      }
      list.sort((a, b) => idxCmp(a.index, b.index));
      instances[f.id] = { list, anchor, originals };
      const word = f.type;
      const named = (st) => list.filter((x) => x.state === st);
      const off = named('off_base'), none = named('changed_nothing'), run = list.filter((x) => x.state !== 'skipped');
      const cut = originals.some((o) => TOOLS[o].cut);
      const baseId = (() => { const i = d.features.findIndex((x) => x.id === f.id); return d.features.slice(0, i).reverse().find((x) => x.type !== 'sketch' && ok(x.id)).id; })();
      const listText = (xs) => { const t = xs.map((x) => idxText(x.index)); return t.length > 1 ? t.slice(0, -1).join(', ') + ' and ' + t[t.length - 1] : t[0]; };
      if (off.length) {
        Object.assign(geo, before); health[f.id] = 'error';
        problems.push({ feature: f.id, severity: 'error', code: 'kernel_failed', title: `${f.name} makes ${off.length + 1} solids`,
          message: `${word} ${f.id} makes ${off.length + 1} solids: the copies of instance${off.length > 1 ? 's' : ''} ${listText(off)} don't touch the base, or meet it only along an edge or at a point`,
          elements: off.map((x) => idxText(x.index)), param: null, fix: 'lower count so every instance touches the base, add them to skip, or move the pattern onto the base', skipThem: word !== 'mirror' });
      } else if (run.length && none.length === run.length) {
        Object.assign(geo, before); health[f.id] = 'error';
        const b = Geo.buildPlate(before);
        problems.push({ feature: f.id, severity: 'error', code: 'kernel_failed', title: `${f.name} ${cut ? 'removes' : 'adds'} nothing`,
          message: `${word} ${f.id} ${cut ? 'removes nothing from' : 'adds nothing to'} its base ${baseId}: the result's volume is the base's, ${A().fmt(b.volume, 6)} mm³ (${b.faces.length} faces, ${b.edges.length} edges, and ${b.verts.length} vertices, against the base's ${b.faces.length}, ${b.edges.length}, and ${b.verts.length})`,
          elements: [], param: null, fix: "if every copy lies inside the base or misses it, change the direction, length, count, or plane; a mirror about the original's own plane of symmetry changes nothing" });
      } else if (none.length) {
        problems.push({ feature: f.id, severity: 'warning', code: 'instances_changed_nothing', title: `${none.length} ${none.length > 1 ? 'copies' : 'copy'} ${cut ? 'cut' : 'add'}${none.length > 1 ? '' : 's'} nothing`,
          message: `${word} ${f.id}: the copies of instance${none.length > 1 ? 's' : ''} ${listText(none)} ${cut ? 'remove' : 'add'} nothing (no face of theirs is in the result)`,
          elements: none.map((x) => idxText(x.index)), param: word === 'mirror' ? 'copies' : 'skip',
          fix: word === 'mirror' ? "take the original out of copies if it isn't meant" : "lower count or add them to skip if they aren't meant; a copy inside the base, or a pocket's copy that misses it, is built exactly as asked", skipThem: word !== 'mirror' });
      }
    }
    return { geo, instances };
  }

  // ───────── The dialogs ─────────
  const firstValues = (type) => type === 'linear_pattern'
    // FreeCAD 1.1.3's LinearPattern: 2 occurrences over a length of 100 mm (m6.0 section 7, *First values*).
    ? { copies: { type: 'features', features: [] }, skip: [], directions: [{ along: null, reversed: false, dist: 'extent', length: 100, count: 2 }, { along: null, reversed: false, dist: 'extent', length: 100, count: 1 }] }
    : type === 'polar_pattern'
      // FreeCAD 1.1.3's PolarPattern: 3 occurrences over 360°.
      ? { copies: { type: 'features', features: [] }, skip: [], axis: null, reversed: false, dist: 'extent', angle: 360, count: 3 }
      : { copies: { type: 'features', features: [] }, plane: null };
  function texts(type, P) {
    if (type === 'linear_pattern') return { q0: String(P.directions[0].count), d0: A().fmt(P.directions[0].length, 6), q1: String(P.directions[1] ? P.directions[1].count : 1), d1: A().fmt(P.directions[1] ? P.directions[1].length : 100, 6) };
    if (type === 'polar_pattern') return { q: String(P.count), ang: A().fmt(P.angle === 360 && P.dist === 'extent' ? 180 : P.angle, 9) };
    return {};
  }
  function nextId(d, c) { let n = 1; while (d.features.some((f) => f.id === c + String(n).padStart(7, '0'))) n++; return c + String(n).padStart(7, '0'); }
  function open(type, featureId) {
    const app = A(); const d = app.doc(); if (!d) return;
    if (S().sketch) { app.toast('Finish or cancel the sketch first'); return; }
    if (d.kind !== 'plate') { app.toast('Open plate-with-holes.silico (Home › Recent) to try patterns: this prototype models them on the plate of the M6 design.'); return; }
    if (S().dialog) app.cancelDialog();
    const f = featureId && d.features.find((x) => x.id === featureId);
    const P = f ? clone(d.pat[f.id]) : firstValues(type);
    if (P.directions && P.directions.length === 1) P.directions.push({ along: null, reversed: false, dist: P.directions[0].dist, length: 100, count: 1 });
    const id = f ? f.id : nextId(d, type === 'mirror' ? 'm' : 't');
    const count = d.features.filter((x) => x.type === type).length + 1;
    const name = f ? f.name : { linear_pattern: 'Rectangular Pattern', polar_pattern: 'Circular Pattern', mirror: 'Mirror' }[type] + count;
    // Edit Feature opens on the feature's values (Full for exactly 360°, Partial for any other extent, Spacing for a spacing).
    let polarDist = null;
    if (type === 'polar_pattern') polarDist = P.dist === 'spacing' ? 'spacing' : P.angle === 360 ? 'full' : 'partial';
    const mixed = type === 'linear_pattern' && P.directions[1].count > 1 && P.directions[0].dist !== P.directions[1].dist;
    S().dialog = { type, pattern: true, featureId: id, isNew: !f, name, draft: P, polarDist, mixed, t: texts(type, P), bad: {},
      openSnap: app.snap(d), openUndo: d.undo.length, openRev: d.rev, activeBox: f ? null : 'objects', suppression: false };
    S().sel = []; S().selFeature = f ? f.id : null;
    app.syncHighlight(); app.render();
  }
  const complete = (D) => {
    const P = D.draft;
    if (!P.copies.features.length && P.copies.type === 'features') return false;
    if (D.type === 'linear_pattern') return !!P.directions[0].along;
    if (D.type === 'polar_pattern') return !!P.axis;
    return !!P.plane;
  };
  // The draft as the document holds it: Axis 2 with a Quantity of 1, or no axis, is left out.
  function docForm(D) {
    const P = clone(D.draft);
    if (D.type === 'linear_pattern') {
      if (!P.directions[1].along || P.directions[1].count <= 1) P.directions = [P.directions[0]];
      else if (!D.mixed) P.directions[1].dist = P.directions[0].dist;
    }
    if (D.type === 'polar_pattern') { if (D.polarDist === 'full') { P.dist = 'extent'; P.angle = 360; } else P.dist = D.polarDist === 'spacing' ? 'spacing' : 'extent'; }
    if (P.copies.type === 'solid') delete P.copies.features;
    return P;
  }
  // Each change is an edit in the dialog's transaction; the preview is the model it rebuilds.
  function apply() {
    const app = A(); const D = S().dialog; if (!D) return;
    if (Object.values(D.bad).some(Boolean)) { app.render(); return; }
    const want = complete(D);
    app.edit((d) => {
      const at = d.features.findIndex((f) => f.id === D.featureId);
      if (want) {
        if (at < 0) {
          const inputs = D.draft.copies.type === 'features' ? [...D.draft.copies.features] : [d.features.filter((f) => f.type !== 'sketch').slice(-1)[0].id];
          d.features.push({ id: D.featureId, type: D.type, name: D.name, inputs });
          S().selFeature = D.featureId; // it joins the timeline selected, as the dialog's feature is
        } else if (d.features[at].type !== D.type) d.features[at].type = D.type; // Type switched while new: replaced at its place
        d.pat[D.featureId] = docForm(D);
      } else if (at >= 0 && D.isNew) { d.features.splice(at, 1); delete d.pat[D.featureId]; }
    }, { noUndo: true, preview: true });
    app.render();
  }
  function ok() {
    const app = A(); const D = S().dialog; const d = app.doc(); if (!D || !d) return;
    if (!complete(D) || Object.values(D.bad).some(Boolean)) return;
    const f = d.features.find((x) => x.id === D.featureId);
    if (f && D.name.trim()) f.name = D.name.trim();
    if (app.snap(d) !== D.openSnap) { d.undo.push({ s: D.openSnap, rev: D.openRev, label: (D.isNew ? 'Add ' : 'Edit ') + D.name }); d.redo = []; }
    S().dialog = null; S().selFeature = D.featureId; app.syncHighlight(); app.render();
    app.toast((D.isNew ? 'Added ' : 'Applied ') + D.name + '. ' + app.KEYNAME.Mod + 'Z undoes it as one step.', 'check');
  }

  function selRow(label, box, D, content, opts = {}) {
    const active = D.activeBox === box;
    return `<div class="prop"><span class="k">${esc(label)}</span><div style="display:flex;gap:4px;min-width:0">
      <div class="selbox multi ${active ? 'active' : ''}" style="flex:1" data-act="activateBox" data-box="${box}" data-tip="Selection box" data-tipd="${esc(opts.tip || '')}">${content || `<span class="subdued">${esc(opts.empty || 'Select')}</span>`}</div>
      ${opts.flip ? `<button class="ab-btn flip ${opts.flipped ? 'on' : ''}" data-act="patFlip" data-k="${opts.flip}" ${opts.flipOff ? 'disabled' : ''} aria-label="Flip ${esc(label)}" aria-pressed="${!!opts.flipped}" data-tip="Flip" data-tipd="Turns the direction round (writes reversed). The arrow in the canvas flips it too">${icon('flip')}</button>` : ''}
    </div></div>`;
  }
  function chipOf(r, k) { return `<span class="chip">${icon(r.word ? (r.word.length === 1 ? 'axis' : 'plane') : r.kind === 'face' ? 'selFace' : 'selEdge')} ${esc(refLabel(r))}</span><span style="flex:1"></span><button class="ab-btn" style="height:18px;min-width:18px" data-act="patClear" data-k="${k}" aria-label="Clear">${icon('close')}</button>`; }
  function dropRow(label, k, value, opts = {}) {
    return `<div class="prop"><span class="k">${esc(label)}</span><button class="field select" data-act="patDrop" data-k="${k}" ${opts.disabled ? 'disabled' : ''} aria-haspopup="menu" data-tip="${esc(opts.tip || label)}" data-tipd="${esc(opts.tipd || '')}">${opts.icon ? `<span style="display:flex;align-items:center;gap:6px">${icon(opts.icon)}${esc(value)}</span>` : esc(value)} ${opts.disabled ? icon('lock') : icon('down')}</button></div>`;
  }
  function numRow(label, k, D, unit, opts = {}) {
    const bad = D.bad[k];
    return `<div class="prop"><span class="k">${esc(label)}</span><div class="field ${bad ? 'bad' : ''} ${opts.disabled ? 'off' : ''}"><input data-key="pat-${k}" data-input="pat" data-k="${k}" value="${esc(D.t[k])}" aria-label="${esc(label)}" inputmode="decimal" ${opts.disabled ? 'disabled' : ''}>${unit ? `<span class="unit">${unit}</span>` : ''}</div></div>
      ${bad ? `<div class="prop"><span></span><span class="err-text" style="font-size:11px;white-space:normal">${esc(bad)}</span></div>` : ''}`;
  }
  function renderDialog(d) {
    const app = A(); const D = S().dialog; const P = D.draft;
    const running = S().rebuild.state === 'running';
    const probs = (d.next ? d.next.problems : d.problems).filter((p) => p.feature === D.featureId);
    let rows = '';
    if (D.type !== 'mirror') rows += dropRow('Type', 'type', TYPES[D.type], { icon: ICON[D.type], disabled: !D.isNew, tip: 'Type', tipd: D.isNew ? 'Rectangular or Circular. Switching keeps Object Type and Objects' : "Fixed in Edit Feature: a pattern's type can't be changed. Delete it and make the other kind" });
    rows += dropRow('Object Type', 'otype', P.copies.type === 'features' ? 'Features' : 'Bodies', { tipd: 'Features copies pads, pockets, revolves, and grooves; Bodies copies the whole body' });
    const objs = P.copies.type === 'solid' ? `<span class="chip">${icon('body')} Body1</span>`
      : P.copies.features.map((o) => { const f = d.features.find((x) => x.id === o); return `<span class="chip">${icon(f ? f.type : 'pocket')} ${esc(f ? f.name : o)}<button class="chip-x" data-act="patRemoveObj" data-id="${o}" aria-label="Remove ${esc(f ? f.name : o)}">${icon('close')}</button></span>`; }).join('');
    rows += selRow('Objects', 'objects', D, objs ? objs + `<span style="flex:1"></span><span class="subdued" style="font-size:11px">${P.copies.type === 'solid' ? 1 : P.copies.features.length}</span>` : '', { empty: P.copies.type === 'solid' ? 'Body1' : 'Select features', tip: 'Pick a pad, pocket, revolve, or groove in the timeline, or one of its faces in the canvas' });
    if (D.type === 'linear_pattern') {
      const [a, b] = P.directions;
      rows += selRow('Axis 1', 'axis0', D, a.along ? chipOf(a.along, 'axis0') : '', { empty: 'Select an axis, edge, or face', flip: 'axis0', flipped: a.reversed, flipOff: !a.along, tip: 'X, Y, or Z under Origin in the browser, or a straight edge or a flat face in the canvas' });
      rows += selRow('Axis 2', 'axis1', D, b.along ? chipOf(b.along, 'axis1') : '', { empty: 'Optional', flip: 'axis1', flipped: b.reversed, flipOff: !b.along, tip: 'Leave it empty, or its Quantity at 1, for a pattern in one direction' });
      const distLabel = (x) => (x === 'extent' ? 'Extent' : 'Spacing');
      if (!D.mixed) rows += dropRow('Distribution', 'dist', distLabel(a.dist), { tipd: 'Extent: Distance from the first instance to the last. Spacing: between neighbours' });
      for (const k of [0, 1]) {
        const x = P.directions[k]; const off = k === 1 && !x.along;
        rows += `<div class="section">Axis ${k + 1}${k === 1 && (!x.along || x.count <= 1) ? ' <span style="text-transform:none;letter-spacing:0;margin-left:4px">· left out</span>' : ''}</div>`;
        if (D.mixed) rows += dropRow('Distribution', 'dist' + k, distLabel(x.dist), { tipd: 'This pattern\'s directions differ in form, as an agent wrote it: each keeps its own' });
        rows += numRow('Quantity', 'q' + k, D, '', { disabled: off });
        rows += numRow('Distance', 'd' + k, D, 'mm', { disabled: off || x.count <= 1 });
      }
    } else if (D.type === 'polar_pattern') {
      rows += selRow('Axis', 'axis', D, P.axis ? chipOf(P.axis, 'axis') : '', { empty: 'Select an axis or an edge', flip: 'axis', flipped: P.reversed, flipOff: !P.axis, tip: 'X, Y, or Z under Origin in the browser, or a straight or circular edge in the canvas' });
      rows += dropRow('Distribution', 'pdist', { full: 'Full', partial: 'Partial', spacing: 'Spacing' }[D.polarDist], { tipd: 'Full: evenly round 360°. Partial: over the Angle. Spacing: the Angle between neighbours' });
      rows += numRow('Quantity', 'q', D, '');
      if (D.polarDist !== 'full') rows += numRow('Angle', 'ang', D, '°');
    } else {
      rows += selRow('Mirror Plane', 'plane', D, P.plane ? chipOf(P.plane, 'plane') : '', { empty: 'Select a plane or a flat face', tip: 'XY, XZ, or YZ under Origin in the browser, or a flat face in the canvas' });
    }
    if (D.type !== 'mirror') rows += `<div class="prop"><span class="k"></span><button class="check ${D.suppression ? 'on' : ''}" data-act="patSuppression" role="checkbox" aria-checked="${D.suppression}" data-tip="Suppression" data-tipd="Shows a box over each instance in the canvas. Clear one to skip that instance">
      <span class="box"><svg viewBox="0 0 10 10"><path d="M2 5.2l2 2 4-4.4"/></svg></span>Suppression</button></div>`;
    const nSkip = (P.skip || []).length;
    const total = D.type === 'mirror' ? 2 : D.type === 'linear_pattern' ? P.directions[0].count * (P.directions[1].along ? P.directions[1].count : 1) : P.count;
    let st;
    if (!complete(D)) st = `<div class="dlg-status">${icon('info')} ${P.copies.type === 'features' && !P.copies.features.length ? 'Select what to copy.' : D.type === 'mirror' ? 'Select the mirror plane.' : 'Select the axis.'} Nothing is previewed until then.</div>`;
    else if (running) st = `<div class="dlg-status running"><span class="spinner"></span>Rebuilding the preview · ${S().rebuild.done} of ${S().rebuild.steps}<span style="flex:1"></span><span class="subdued">Esc stops it</span></div>`;
    else if (S().rebuild.state === 'stopped') st = `<div class="dlg-status">${icon('stop')} Preview stopped. Rebuild ${app.kbd(['Mod', 'B'])} resumes it.</div>`;
    else if (probs.some((p) => p.severity === 'error')) st = `<div class="dlg-status" style="color:var(--red)">${icon('error')} The preview has an error. OK keeps the feature with it.</div>`;
    else st = `<div class="dlg-status">${icon('check')} ${D.type === 'mirror' ? 'The original and its mirror image' : `${total} instance${total === 1 ? '' : 's'}${nSkip ? `, ${nSkip} skipped` : ''}, the original counted`} · revision ${d.rev} · ${S().rebuild.ms} ms</div>`;
    const okDisabled = !complete(D) || Object.values(D.bad).some(Boolean);
    return `<div class="panel-title">${icon(ICON[D.type])}<span>${TITLE[D.type]}</span><span class="grow"></span><button class="ab-btn" data-act="cancelDialog" aria-label="Cancel" data-tip="Cancel" data-tipk="Esc">${icon('close')}</button></div>
      <div class="panel-body">
        <div class="prop"><span class="k">Name</span><div class="field"><input data-key="pat-name" data-input="pat" data-k="name" value="${esc(D.name)}" aria-label="Feature name"></div></div>
        ${A().sectionHtml('Inputs')}${rows}
        ${A().sectionHtml('Preview')}${st}
        ${running || !complete(D) ? '' : probs.map((x) => A().errCard(x, { noEdit: true, noShow: true })).join('')}
        ${P.copies.type === 'solid' ? '<div class="hint-block">Prototype: the stand-in model draws a feature\'s copies only. In the app, Bodies copies the whole body.</div>' : ''}
        ${D.activeBox ? `<div class="hint-block">${{ objects: 'The <b>Objects</b> box is active (blue). Click a feature in the timeline, or one of its faces in the canvas.', axis0: '<b>Axis 1</b> is active. Click X, Y, or Z under Origin in the browser, or an edge or a flat face.', axis1: '<b>Axis 2</b> is active. Click X, Y, or Z under Origin, or an edge or a flat face.', axis: '<b>Axis</b> is active. Click X, Y, or Z under Origin, or a straight or circular edge.', plane: '<b>Mirror Plane</b> is active. Click XY, XZ, or YZ under Origin, or a flat face.' }[D.activeBox]}</div>` : ''}
        ${D.suppression ? '<div class="hint-block">Each instance has a box in the canvas. Clear one to skip it; the original has none.</div>' : ''}
      </div>
      <div class="dlg-footer"><button class="btn btn-secondary" data-act="cancelDialog">Cancel <span class="kbd">Esc</span></button><button class="btn btn-primary" data-act="okDialog" ${okDisabled ? 'disabled' : ''}>OK <span class="kbd">↵</span></button></div>`;
  }

  // ───────── Dialog inputs ─────────
  function input(k, v) {
    const D = S().dialog; if (!D || !D.pattern) return;
    D.t[k] = v;
    if (k === 'name') { D.name = v; return; }
    const P = D.draft; const s = v.trim();
    const whole = /^\d+$/.test(s) ? Number(s) : NaN;
    if (/^q\d?$/.test(k)) {
      if (!(whole >= 1)) { D.bad[k] = `"${s}" isn't a whole number of instances. The original counts as one.`; A().render(); return; }
      const product = D.type === 'linear_pattern' ? whole * (k === 'q0' ? (P.directions[1].along ? P.directions[1].count : 1) : P.directions[0].count) : whole;
      if (product > 1000) { D.bad[k] = 'At most 1,000 instances in all, the original included.'; A().render(); return; }
      if (k === 'q' && D.polarDist === 'spacing' && P.angle * (whole - 1) >= 360) { D.bad[k] = `${whole} instances ${A().fmt(P.angle)}° apart reach a whole turn or more. Use Full, or fewer.`; A().render(); return; }
      D.bad[k] = null; if (k === 'q') D.bad.ang = null;
      if (k === 'q') P.count = whole; else P.directions[+k[1]].count = whole;
    } else if (/^d\d$/.test(k)) {
      const r = A().parseValue(v, 'The distance'); D.bad[k] = r.err || null; if (r.err) { A().render(); return; }
      P.directions[+k[1]].length = r.v;
    } else if (k === 'ang') {
      const n = Number(s);
      if (!/^[+-]?(\d+\.?\d*|\.\d+)$/.test(s)) D.bad[k] = `"${s}" isn't a number. Type a plain number of degrees, like 90.`;
      else if (!(n > 0)) D.bad[k] = 'The angle must be more than 0°.';
      else if (D.polarDist === 'partial' && n >= 360) D.bad[k] = 'Partial is under 360°: Full spreads the instances round a whole turn.';
      else if (D.polarDist === 'spacing' && n * (P.count - 1) >= 360) D.bad[k] = `${P.count} instances ${A().fmt(n)}° apart reach a whole turn or more. Use Full, or fewer.`;
      else D.bad[k] = null;
      if (D.bad[k]) { A().render(); return; }
      P.angle = n;
    }
    apply();
  }
  function dropMenu(el, k) {
    const D = S().dialog; const P = D.draft; const r = el.getBoundingClientRect();
    const tick = (on) => (on ? '✓ ' : '   ');
    let items = [];
    if (k === 'type') items = ['linear_pattern', 'polar_pattern'].map((t) => ({ label: tick(D.type === t) + TYPES[t], icon: ICON[t], run: () => switchType(t) }));
    if (k === 'otype') items = [['features', 'Features'], ['solid', 'Bodies']].map(([t, l]) => ({ label: tick(P.copies.type === t) + l, run: () => { P.copies = t === 'solid' ? { type: 'solid', features: [] } : { type: 'features', features: [] }; D.activeBox = t === 'features' ? 'objects' : D.activeBox; apply(); } }));
    if (k === 'dist' || k === 'dist0' || k === 'dist1') items = [['extent', 'Extent'], ['spacing', 'Spacing']].map(([t, l]) => ({ label: tick((k === 'dist1' ? P.directions[1] : P.directions[0]).dist === t) + l, run: () => {
      if (k === 'dist') P.directions.forEach((x) => { x.dist = t; }); else P.directions[+k[4]].dist = t; apply(); } }));
    if (k === 'pdist') items = [['full', 'Full'], ['partial', 'Partial'], ['spacing', 'Spacing']].map(([t, l]) => ({ label: tick(D.polarDist === t) + l, run: () => {
      D.polarDist = t; if (t !== 'full') { const n = Number(D.t.ang); P.angle = n > 0 && n < 360 ? n : t === 'spacing' ? 30 : 180; D.t.ang = A().fmt(P.angle, 9); D.bad.ang = null; } apply(); } }));
    PAT_menu(r.left, r.bottom + 2, items);
  }
  let PAT_menu = () => {};
  function switchType(t) {
    const D = S().dialog; if (!D.isNew || D.type === t) return;
    const keep = D.draft.copies; const P = firstValues(t); P.copies = keep;
    D.type = t; D.draft = P; D.t = texts(t, P); D.bad = {}; D.polarDist = t === 'polar_pattern' ? 'full' : null;
    D.name = (t === 'linear_pattern' ? 'Rectangular Pattern' : 'Circular Pattern') + (A().doc().features.filter((x) => x.type === t).length + 1);
    D.activeBox = keep.type === 'features' && !keep.features.length ? 'objects' : t === 'linear_pattern' ? 'axis0' : 'axis';
    apply();
  }
  function flip(k) {
    const D = S().dialog; const P = D.draft;
    if (k === 'axis') P.reversed = !P.reversed; else P.directions[+k[4]].reversed = !P.directions[+k[4]].reversed;
    apply();
  }
  function clearBox(k) {
    const D = S().dialog; const P = D.draft;
    if (k === 'axis') P.axis = null; else if (k === 'plane') P.plane = null; else P.directions[+k[4]].along = null;
    D.activeBox = k; apply();
  }
  function removeObj(id) { const D = S().dialog; D.draft.copies.features = D.draft.copies.features.filter((x) => x !== id); D.activeBox = 'objects'; apply(); }

  // A pick while a pattern's dialog is open: into the active box if it fits there, else the first
  // empty box it fits (an origin axis can only be an axis, a feature only an object).
  function pick(what) {
    const app = A(); const D = S().dialog; const d = app.doc(); const P = D.draft;
    const boxes = D.type === 'linear_pattern' ? ['axis0', 'axis1'] : D.type === 'polar_pattern' ? ['axis'] : ['plane'];
    const fitsBox = (b) => {
      if (b === 'objects') return what.feature != null && (D.activeBox === 'objects' || !P.copies.features.length || what.kind == null);
      if (b === 'plane') return what.word ? what.word.length === 2 : what.kind === 'face' && what.ref.kind === 'plane';
      if (b === 'axis') return what.word ? what.word.length === 1 : what.kind === 'edge';
      return what.word ? what.word.length === 1 : (what.kind === 'edge' && what.ref.kind === 'line') || (what.kind === 'face' && what.ref.kind === 'plane');
    };
    const get = (b) => (b === 'objects' ? null : b === 'axis' ? P.axis : b === 'plane' ? P.plane : P.directions[+b[4]].along);
    let box = D.activeBox && fitsBox(D.activeBox) ? D.activeBox : ['objects', ...boxes].find((b) => fitsBox(b) && !get(b)) || ['objects', ...boxes].find(fitsBox);
    if (!box) {
      if (what.kind === 'edge' && D.type === 'linear_pattern') app.toast(`${what.id} is a circle: a direction is a straight edge or a flat face. A circle is a Circular Pattern's axis.`, 'warn', 'err');
      else if (what.word) app.toast(`${refLabel({ word: what.word })} can't go here: ${D.type === 'mirror' ? 'a mirror takes a plane' : 'this box takes an axis'}.`, 'warn', 'err');
      else app.toast(`A ${what.ref && what.ref.kind === 'cylinder' ? 'curved face' : what.kind} can't go in ${D.type === 'mirror' ? 'Mirror Plane: pick a plane or a flat face' : 'an axis box'}.`, 'warn', 'err');
      return;
    }
    if (box === 'objects') {
      const f = d.features.find((x) => x.id === what.feature);
      if (P.copies.type === 'solid') { app.toast('Object Type is Bodies: Body1 is already the object. Switch it to Features to pick features.'); return; }
      if (!f || !SWEEPS.includes(f.type)) {
        const kind = f ? (A().FEATURE_KIND[f.type] || f.type).toLowerCase() : 'feature';
        app.toast(`${f ? f.name : what.feature} is a ${kind}: a pattern copies pads, pockets, revolves, and grooves.${f && isPattern(f.type) ? ' Pick its original, or set Object Type to Bodies.' : ''}`, 'warn', 'err');
        return;
      }
      const at = d.features.findIndex((x) => x.id === D.featureId);
      if (!D.isNew && at >= 0 && d.features.findIndex((x) => x.id === f.id) > at) { app.toast(`${f.name} comes after ${D.name}: a pattern copies features before it.`, 'warn', 'err'); return; }
      const has = P.copies.features.includes(f.id);
      P.copies.features = has ? P.copies.features.filter((x) => x !== f.id) : [...P.copies.features, f.id];
      if (!has && !get(boxes[0])) D.activeBox = boxes[0];
      if (has) app.toast(f.name + ' taken out of Objects');
    } else {
      let r;
      if (what.word) r = { word: what.word };
      else if (what.kind === 'edge' && what.ref.kind === 'line') { const [a, b] = [what.ref.pts[0], what.ref.pts[what.ref.pts.length - 1]]; const v = b.clone().sub(a).normalize(); r = { name: what.ref.stable, kind: 'edge', label: what.id + ' (edge)', vec: v.toArray().map((x) => R(x)), point: a.toArray().map(R) }; }
      else if (what.kind === 'edge') r = { name: what.ref.stable, kind: 'edge', label: what.id + ' (circle)', vec: [0, 0, 1], point: [what.ref.center[0], what.ref.center[1], 0] };
      else { const f = what.ref; const o = f.frame.o; r = { name: f.stable, kind: 'face', label: what.id + ' (face)', vec: f.normal, normal: f.normal, origin: [o.x, o.y, o.z].map(R) }; }
      if (box === 'axis') P.axis = r; else if (box === 'plane') P.plane = r; else P.directions[+box[4]].along = r;
      D.activeBox = null; // a single pick fills its box; the next goes to the first empty box it fits
    }
    apply();
  }

  // Skip them: the warning's instances written into skip (m6.0 section 7, M6.7).
  function skipThem(fid) {
    const app = A(); const d = app.doc(); const D = S().dialog;
    const inst = d.next && d.next.instances && d.next.instances[fid]; if (!inst) return;
    const add = inst.list.filter((x) => x.state === 'changed_nothing' || x.state === 'off_base').map((x) => x.index);
    if (!add.length) return;
    const merge = (P) => { P.skip = [...(P.skip || []), ...add].sort(idxCmp); };
    if (D && D.pattern && D.featureId === fid) { merge(D.draft); apply(); app.toast(`Skipped ${add.length} instance${add.length > 1 ? 's' : ''}`); return; }
    const name = app.featName(d, fid);
    app.edit((dd) => { merge(dd.pat[fid]); }, { label: `${name}: skip ${add.map(idxText).join('; ')}` });
    app.toast(`${name} skips ${add.map(idxText).join('; ')} now. ${app.KEYNAME.Mod}Z puts them back.`, 'check');
  }
  function toggleSkip(fid, ix) {
    const D = S().dialog; if (!D || D.featureId !== fid) return;
    const P = D.draft; const t = idxText(ix);
    P.skip = skipped(P, ix) ? P.skip.filter((s) => idxText(s) !== t) : [...(P.skip || []), ix].sort(idxCmp);
    apply();
  }

  // ───────── The canvas: instance boxes, marks, flip arrows, the mirror plane ─────────
  let hits = [];
  function shown() {
    const D = S().dialog; const d = A().doc(); if (!d || d.kind !== 'plate') return null;
    if (D && D.pattern) return { id: D.featureId, dialog: D };
    const id = S().selFeature || S().hoverFeature; const f = id && d.features.find((x) => x.id === id);
    return f && isPattern(f.type) ? { id, dialog: null } : null;
  }
  function overlay() {
    hits = [];
    const sh = shown(); const d = A().doc(); if (!sh) return '';
    const inst = d.next && d.next.instances && d.next.instances[sh.id];
    const P = sh.dialog ? sh.dialog.draft : d.pat[sh.id]; if (!P) return '';
    const type = sh.dialog ? sh.dialog.type : d.features.find((x) => x.id === sh.id).type;
    let s = '';
    const pr = (p) => VP.project(new T.Vector3(...p));
    // The mirror plane, as a square about the original's anchor, in the selection's blue.
    if (type === 'mirror' && P.plane && inst && inst.anchor) {
      const pl = P.plane.word ? PLANES[P.plane.word] : { origin: P.plane.origin, normal: P.plane.normal };
      const n = new T.Vector3(...pl.normal); const o = new T.Vector3(...pl.origin);
      const a0 = new T.Vector3(...inst.anchor); const c = a0.clone().sub(n.clone().multiplyScalar(a0.clone().sub(o).dot(n)));
      const u = Math.abs(n.z) > 0.9 ? new T.Vector3(1, 0, 0) : new T.Vector3(0, 0, 1); const v = new T.Vector3().crossVectors(n, u);
      const sz = 36; const pts = [[-1, -1], [1, -1], [1, 1], [-1, 1]].map(([i, j]) => pr(c.clone().addScaledVector(u, i * sz * (Math.abs(n.z) > 0.9 ? 1.6 : 0.5)).addScaledVector(v, j * sz)));
      s += `<polygon points="${pts.map((p) => p.x.toFixed(1) + ',' + p.y.toFixed(1)).join(' ')}" fill="#186add" fill-opacity=".16" stroke="#4676ff" stroke-width="1" stroke-dasharray="5 3"/>`;
    }
    // Flip arrows from the original's anchor, along each direction (a turn's sense for a circular pattern).
    if (sh.dialog && inst && inst.anchor) {
      const a = new T.Vector3(...inst.anchor); const A0 = pr(a);
      const arrow = (dir, k, label) => {
        const b = pr(a.clone().addScaledVector(new T.Vector3(...dir), 10)); const L = Math.hypot(b.x - A0.x, b.y - A0.y); if (L < 2) return '';
        const ux = (b.x - A0.x) / L, uy = (b.y - A0.y) / L, tip = { x: A0.x + ux * 48, y: A0.y + uy * 48 };
        hits.push({ kind: 'flip', k, x: tip.x, y: tip.y, r: 12 });
        return `<line x1="${A0.x.toFixed(1)}" y1="${A0.y.toFixed(1)}" x2="${tip.x.toFixed(1)}" y2="${tip.y.toFixed(1)}" stroke="#4676ff" stroke-width="2.2"/>
          <path d="M${(tip.x + ux * 11).toFixed(1)},${(tip.y + uy * 11).toFixed(1)} L${(tip.x - uy * 5.5).toFixed(1)},${(tip.y + ux * 5.5).toFixed(1)} L${(tip.x + uy * 5.5).toFixed(1)},${(tip.y - ux * 5.5).toFixed(1)} Z" fill="#4676ff"/>
          <rect x="${(tip.x + ux * 18 - 8).toFixed(1)}" y="${(tip.y + uy * 18 - 8).toFixed(1)}" width="16" height="16" rx="3" fill="#212121" stroke="#4676ff"/><text x="${(tip.x + ux * 18).toFixed(1)}" y="${(tip.y + uy * 18 + 3.8).toFixed(1)}" fill="#fff" font-size="10.5" text-anchor="middle" font-family="Inter Silico, Inter">${label}</text>`;
      };
      if (type === 'linear_pattern') P.directions.forEach((x, k) => { if (x.along && (k === 0 || x.count > 1)) s += arrow(dirOf(x.along, x.reversed), 'axis' + k, k + 1); });
      if (type === 'polar_pattern' && P.axis) {
        const ax = P.axis.word ? { point: [0, 0, 0], dir: WORDS[P.axis.word] } : { point: P.axis.point, dir: P.axis.vec };
        const p0 = new T.Vector3(...ax.point), dv = new T.Vector3(...ax.dir);
        const q0 = pr(p0.clone().addScaledVector(dv, -8)), q1 = pr(p0.clone().addScaledVector(dv, 26));
        s += `<line x1="${q0.x.toFixed(1)}" y1="${q0.y.toFixed(1)}" x2="${q1.x.toFixed(1)}" y2="${q1.y.toFixed(1)}" stroke="#4676ff" stroke-width="1.2" stroke-dasharray="6 3 1.5 3"/>`;
        // The turn's sense: the tangent at the original's anchor, right-handed about the axis (left-handed reversed).
        const rad = a.clone().sub(p0); rad.sub(dv.clone().multiplyScalar(rad.dot(dv)));
        if (rad.length() > 1e-6) s += arrow(new T.Vector3().crossVectors(dv, rad).normalize().multiplyScalar(P.reversed ? -1 : 1).toArray(), 'axis', '↻');
      }
    }
    if (!inst || !inst.list) return s;
    const box = sh.dialog && sh.dialog.suppression && type !== 'mirror';
    for (const x of inst.list) {
      const p = pr(x.anchor); if (p.z > 1 || p.z < -1) continue;
      if (box) {
        const on = x.state !== 'skipped';
        s += `<g><rect x="${(p.x - 6.5).toFixed(1)}" y="${(p.y - 6.5).toFixed(1)}" width="13" height="13" rx="3" fill="${on ? '#186add' : '#171717'}" stroke="${on ? '#186add' : '#525151'}" stroke-width="1"/>${on ? `<path d="M${(p.x - 3.8).toFixed(1)},${(p.y + 0.3).toFixed(1)} l2.5,2.5 l4.8,-5.2" stroke="#fff" stroke-width="1.8" fill="none" stroke-linecap="round" stroke-linejoin="round"/>` : ''}</g>`;
        hits.push({ kind: 'skip', index: x.index, x: p.x, y: p.y, r: 9 });
      } else if (x.state === 'skipped') {
        s += `<circle cx="${p.x.toFixed(1)}" cy="${p.y.toFixed(1)}" r="4" fill="none" stroke="#9f9f9f" stroke-width="1.2" stroke-dasharray="2 2"/>`;
      }
      const mk = box ? { x: p.x + 15, y: p.y - 1 } : p;
      if (x.state === 'changed_nothing') s += `<g transform="translate(${(mk.x - 8).toFixed(1)},${(mk.y - 8).toFixed(1)})"><rect width="16" height="16" rx="3" fill="#2a1d0b" stroke="#ff7a0c"/><g transform="translate(2,2) scale(0.6)" stroke="#ff7a0c" fill="none" stroke-width="2.4" stroke-linecap="round">${ICONS.warn}</g></g>`;
      if (x.state === 'off_base') s += `<g transform="translate(${(mk.x - 8).toFixed(1)},${(mk.y - 8).toFixed(1)})"><rect width="16" height="16" rx="3" fill="#2a0a12" stroke="#ff3865"/><g transform="translate(2,2) scale(0.6)" stroke="#ff3865" fill="none" stroke-width="2.4" stroke-linecap="round">${ICONS.error}</g></g>`;
      if (x.state === null) s += `<text x="${mk.x.toFixed(1)}" y="${(mk.y + 4).toFixed(1)}" fill="#939090" font-size="11" text-anchor="middle" font-family="Inter Silico, Inter">?</text>`;
      if ((box || x.state === 'changed_nothing' || x.state === 'off_base') && sh.dialog) {
        const lbl = idxText(x.index); const lx = box ? p.x - 7 : mk.x - 8;
        s += `<text x="${lx.toFixed(1)}" y="${(p.y + (box ? 20 : 21)).toFixed(1)}" fill="#939090" font-size="10" font-family="JetBrains Mono, monospace">${lbl}</text>`;
      }
    }
    return s;
  }
  function pointerDown(e) {
    if (!hits.length) return false;
    const r = document.getElementById('gl').getBoundingClientRect(); const m = { x: e.clientX - r.left, y: e.clientY - r.top };
    const h = hits.find((x) => Math.hypot(m.x - x.x, m.y - x.y) <= x.r); if (!h) return false;
    const sh = shown(); if (!sh || !sh.dialog) return false;
    if (h.kind === 'flip') flip(h.k); else toggleSkip(sh.id, h.index);
    return true;
  }
  function overHit(e) {
    if (!hits.length) return false;
    const r = document.getElementById('gl').getBoundingClientRect(); const m = { x: e.clientX - r.left, y: e.clientY - r.top };
    return hits.some((x) => Math.hypot(m.x - x.x, m.y - x.y) <= x.r);
  }

  // ───────── Panels: a pattern's properties, a copy's name ─────────
  function featureRows(d, f) {
    const prop = A().propHtml; const P = d.pat[f.id]; if (!P) return '';
    const link = (id) => `<span class="link" data-act="selectFeature" data-id="${id}">${esc(A().featName(d, id))}</span>`;
    let rows = prop('Objects', P.copies.type === 'solid' ? 'Body1 (the whole body)' : P.copies.features.map(link).join(', '));
    const inst = d.next && d.next.instances && d.next.instances[f.id];
    if (f.type === 'linear_pattern') {
      P.directions.forEach((x, k) => { rows += prop('Axis ' + (k + 1), `${esc(refLabel(x.along))}${x.reversed ? ' · reversed' : ''}`); rows += prop('', `${x.count} · ${x.dist === 'extent' ? 'Extent' : 'Spacing'} ${A().fmt(x.length)} mm`); });
    } else if (f.type === 'polar_pattern') {
      rows += prop('Axis', `${esc(refLabel(P.axis))}${P.reversed ? ' · reversed' : ''}`);
      rows += prop('Distribution', P.dist === 'spacing' ? `Spacing · ${A().fmt(P.angle)}°` : P.angle === 360 ? 'Full · 360°' : `Partial · ${A().fmt(P.angle)}°`) + prop('Quantity', P.count);
    } else rows += prop('Mirror Plane', esc(refLabel(P.plane)));
    if (f.type !== 'mirror') {
      const all = f.type === 'linear_pattern' ? P.directions.reduce((s, x) => s * x.count, 1) : P.count;
      rows += prop('Instances', `${all}${f.type === 'linear_pattern' && P.directions.length > 1 ? ` (${P.directions.map((x) => x.count).join(' × ')})` : ''}, the original counted`);
      rows += prop('Skipped', (P.skip || []).length ? `<span class="mono">${P.skip.map((s) => '[' + s.join(', ') + ']').join(' ')}</span>` : 'None');
      if (inst && inst.list) { const n = inst.list.filter((x) => x.state === 'changed_nothing').length; if (n) rows += prop('Changed nothing', `<span class="warn-text">${n}, marked in the canvas</span>`); }
    }
    return rows;
  }
  function plateRows(d, f) {
    const prop = A().propHtml;
    if (f.type === 'sketch') { const r = PLATE_SKETCH_ROWS[f.id] || ['XY', '—', 'None']; return prop('Plane', r[0]) + prop('Definition', '<span style="color:var(--sk-under)">Under defined</span>') + prop('Curves', r[1]) + prop('Dimensions', r[2]); }
    const sk = f.inputs.find((x) => x.startsWith('s'));
    if (f.type === 'pad') return prop('Profile', `<span class="link" data-act="selectFeature" data-id="${sk}">${esc(A().featName(d, sk))}</span> · 1 region`) + prop('Extent', `Distance · ${f.id === 'p0000001' ? 5 : 10} mm`) + prop('Direction', 'Normal (+Z)');
    if (f.type === 'pocket') return prop('Profile', `<span class="link" data-act="selectFeature" data-id="${sk}">${esc(A().featName(d, sk))}</span> · 1 region`) + prop('Extent', 'Through all') + prop('Direction', 'Reversed (+Z)') + prop('Base', esc(A().featName(d, f.inputs[0])));
    return '';
  }
  // A copy's face or edge: its instance and original, read from its name (scheme 6, N10).
  function copyRows(ref) {
    const m = /^(\w{8})\.(linear_pattern|polar_pattern|mirror)\.(?:Face|Edge)\(([CS])([\d,]+) (\w{8})\.(\w+)\./.exec(ref.stable || '')
      || /G (\w{8})\.(linear_pattern|polar_pattern|mirror)\.Face\(([CS])([\d,]+) (\w{8})\.(\w+)\./.exec(ref.stable || '');
    if (!m) return '';
    const d = A().doc(); const prop = A().propHtml;
    const link = (id) => `<span class="link" data-act="selectFeature" data-id="${id}">${esc(A().featName(d, id))}</span>`;
    return A().sectionHtml('Copy') + prop('Instance', `<span class="mono">${m[3]}${m[4]}</span> <span class="subdued">· ${m[4].includes(',') ? '(' + m[4].replace(',', ', ') + ')' : m[2] === 'mirror' ? 'the mirror image' : 'step ' + m[4]}</span>`)
      + prop('Copied by', link(m[1])) + prop('Original', link(m[5]));
  }

  // ───────── Prototype states ─────────
  function openPlate(fresh) {
    const S0 = S(); let d = S0.docs.find((x) => x.kind === 'plate');
    if (d && fresh) { S0.docs = S0.docs.filter((x) => x !== d); d = null; }
    if (!d) { d = A().makeDoc('plate', 'plate-with-holes.silico', '~/Desktop/silico-samples/plate-with-holes.silico'); S0.docs.push(d); }
    window.PROTO.activate(d);
    return d;
  }
  const at = (ms, fn) => setTimeout(fn, ms);
  function protoState(k) {
    const app = A();
    if (k === '8') { openPlate(true); at(300, () => { const r = document.querySelector('[data-act="createMenu"]').getBoundingClientRect(); window.PROTO.createMenu(r); }); return true; }
    if (k === '8a') { openPlate(true); at(300, () => { open('linear_pattern', 't0000001'); S().dialog.suppression = true; app.render(); }); return true; }
    if (k === '8b') {
      openPlate(true);
      at(300, () => { open('linear_pattern', null); pick({ feature: 'q0000002' }); pick({ word: 'x' }); });
      return true;
    }
    if (k === '8h') {
      openPlate(true);
      at(300, () => { open('linear_pattern', null); pick({ feature: 'q0000002' }); pick({ word: 'x' }); input('q0', '3'); });
      at(900, () => { S().dialog.suppression = true; app.render(); });
      return true;
    }
    if (k === '8c') { openPlate(true); at(300, () => { open('polar_pattern', 't0000002'); const D = S().dialog; D.polarDist = 'partial'; D.t.ang = '270'; D.draft.angle = 270; D.t.q = '4'; D.draft.count = 4; D.suppression = true; D.draft.skip = [[2]]; apply(); }); return true; }
    if (k === '8d') { openPlate(true); at(300, () => { open('mirror', 'm0000001'); }); return true; }
    if (k === '8g') { openPlate(true); at(300, () => { open('mirror', null); pick({ feature: 'q0000003' }); pick({ word: 'xz' }); }); return true; }
    if (k === '8e') {
      openPlate(true);
      at(350, () => { const m = VP.model(); const f = m.faces.find((x) => /C2,1 q0000002/.test(x.stable)); if (f) app.setSel([{ kind: 'face', id: f.id }]); S().hoverFeature = null; });
      return true;
    }
    if (k === '8f') { openPlate(true); at(300, () => { S().selFeature = 't0000001'; app.edit((dd) => { dd.pat.t0000001.directions[0].count = 4; dd.pat.t0000001.directions[0].dist = 'spacing'; dd.pat.t0000001.directions[0].length = 40; }, { label: 'Corner holes: 4 along X' }); }); return true; }
    return false;
  }

  window.PAT = { isPattern, TYPES, ICON, plateFeatures, platePatterns, children, evaluate, open, renderDialog, ok, input, dropMenu, flip, clearBox, removeObj, pick, skipThem, overlay, pointerDown, overHit, featureRows, plateRows, copyRows, protoState, openPlate,
    setMenu(fn) { PAT_menu = fn; }, toggleSuppression() { const D = S().dialog; if (D && D.pattern) { D.suppression = !D.suppression; A().render(); } },
    activate(box) { const D = S().dialog; if (D) { D.activeBox = box; A().render(); } },
    defaultGeo() { const d = { kind: 'plate', features: plateFeatures(), pat: platePatterns() }; const health = {}; d.features.forEach((f) => { health[f.id] = 'healthy'; }); return evaluate(d, health, []).geo; } };
})();
