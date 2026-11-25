/* The bracket of docs/design/m5.0-app.md section 7, built face by face so each face
   is its own mesh and pick target, as silico-render meshes it (one mesh and instance
   per face). Element names are illustrative, in Silico's form. */
(function () {
  const V3 = (x, y, z) => new THREE.Vector3(x, y, z);

  // A planar face from a 2D shape placed in a plane: origin o, in-plane axes U and V.
  function planeFace(shape, o, U, V) {
    const g = new THREE.ShapeGeometry(shape, 24);
    const N = new THREE.Vector3().crossVectors(U, V);
    const m = new THREE.Matrix4().makeBasis(U, V, N).setPosition(o);
    g.applyMatrix4(m);
    return { geom: g, frame: { o: o.clone(), U: U.clone(), V: V.clone(), N } };
  }
  function rectShape(u0, v0, u1, v1, holes) {
    const s = new THREE.Shape();
    s.moveTo(u0, v0); s.lineTo(u1, v0); s.lineTo(u1, v1); s.lineTo(u0, v1); s.closePath();
    for (const h of holes || []) {
      const p = new THREE.Path(); p.absarc(h[0], h[1], h[2], 0, Math.PI * 2, true); s.holes.push(p);
    }
    return s;
  }
  function area(g) {
    const p = g.attributes.position, idx = g.index; let a = 0;
    const A = new THREE.Vector3(), B = new THREE.Vector3(), C = new THREE.Vector3();
    const n = idx ? idx.count : p.count;
    for (let i = 0; i < n; i += 3) {
      const ia = idx ? idx.getX(i) : i, ib = idx ? idx.getX(i + 1) : i + 1, ic = idx ? idx.getX(i + 2) : i + 2;
      A.fromBufferAttribute(p, ia); B.fromBufferAttribute(p, ib); C.fromBufferAttribute(p, ic);
      a += B.sub(A).cross(C.sub(A)).length() / 2;
    }
    return a;
  }
  function circlePts(c, axisU, axisV, r, n = 48) {
    const pts = [];
    for (let i = 0; i <= n; i++) {
      const t = (i / n) * Math.PI * 2;
      pts.push(c.clone().addScaledVector(axisU, Math.cos(t) * r).addScaledVector(axisV, Math.sin(t) * r));
    }
    return pts;
  }
  function polyLength(pts) { let l = 0; for (let i = 1; i < pts.length; i++) l += pts[i].distanceTo(pts[i - 1]); return l; }

  // p: {W, H, tb, tu, D, r, baseHoles, uprightHoles}
  function buildBracket(p) {
    const { W, H, tb, tu, D } = p;
    const r = p.r > 0 ? p.r : 0;
    const hr = 2.75;
    const faces = [], edges = [], verts = [];
    const X = V3(1, 0, 0), Y = V3(0, 1, 0), Z = V3(0, 0, 1);
    const fits = (c) => c - hr >= 0 && c + hr <= D;
    const baseHoles = p.baseHoles ? [[40, 12], [40, 28]].filter((h) => fits(h[1])) : [];
    const upHoles = p.uprightHoles ? [[12, 35], [28, 35]].filter((h) => fits(h[0])) : [];

    // The profile in (x, z), as sketch "Bracket profile" draws it.
    const prof = new THREE.Shape();
    prof.moveTo(0, 0); prof.lineTo(W, 0); prof.lineTo(W, tb);
    if (r > 0) { prof.lineTo(tu + r, tb); prof.absarc(tu + r, tb + r, r, -Math.PI / 2, -Math.PI, true); }
    else prof.lineTo(tu, tb);
    prof.lineTo(tu, H); prof.lineTo(0, H); prof.closePath();

    const add = (name, stable, maker, kind, built, extra) => {
      faces.push(Object.assign({ id: name, stable, maker, kind, geom: built.geom, frame: built.frame, area: area(built.geom) }, extra || {}));
    };
    add('Face1', 'p0000001.pad.Face(Start)', 'p0000001', 'plane', planeFace(prof, V3(0, 0, 0), X, Z), { normal: [0, -1, 0] });
    add('Face2', 'p0000001.pad.Face(End)', 'p0000001', 'plane', planeFace(prof, V3(0, D, 0), X, Z), { normal: [0, 1, 0] });
    add('Face3', 'p0000001.pad.Face(G s0000001.na000001)', 'p0000001', 'plane',
      planeFace(rectShape(0, 0, W, D, baseHoles.map((h) => [h[0], h[1], hr])), V3(0, 0, 0), X, Y), { normal: [0, 0, -1] });
    add('Face4', 'p0000001.pad.Face(G s0000001.na000002)', 'p0000001', 'plane',
      planeFace(rectShape(0, 0, D, tb), V3(W, 0, 0), Y, Z), { normal: [1, 0, 0] });
    add('Face5', 'p0000001.pad.Face(G s0000001.na000003)', 'p0000001', 'plane',
      planeFace(rectShape(tu + r, 0, W, D, baseHoles.map((h) => [h[0], h[1], hr])), V3(0, 0, tb), X, Y), { normal: [0, 0, 1] });
    add('Face6', 'p0000001.pad.Face(G s0000001.na000004)', 'p0000001', 'plane',
      planeFace(rectShape(0, tb + r, D, H, upHoles.map((h) => [h[0], h[1], hr])), V3(tu, 0, 0), Y, Z), { normal: [1, 0, 0] });
    add('Face7', 'p0000001.pad.Face(G s0000001.na000005)', 'p0000001', 'plane',
      planeFace(rectShape(0, 0, tu, D), V3(0, 0, H), X, Y), { normal: [0, 0, 1] });
    add('Face8', 'p0000001.pad.Face(G s0000001.na000006)', 'p0000001', 'plane',
      planeFace(rectShape(0, 0, D, H, upHoles.map((h) => [h[0], h[1], hr])), V3(0, 0, 0), Y, Z), { normal: [-1, 0, 0] });

    if (r > 0) {
      const n = 24, pos = [], nor = [], idx = [];
      const cx = tu + r, cz = tb + r;
      for (let i = 0; i <= n; i++) {
        const t = -Math.PI / 2 - (i / n) * (Math.PI / 2);
        const x = cx + r * Math.cos(t), z = cz + r * Math.sin(t);
        pos.push(x, 0, z, x, D, z);
        nor.push(-Math.cos(t), 0, -Math.sin(t), -Math.cos(t), 0, -Math.sin(t));
        if (i < n) { const a = i * 2; idx.push(a, a + 2, a + 1, a + 1, a + 2, a + 3); }
      }
      const g = new THREE.BufferGeometry();
      g.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
      g.setAttribute('normal', new THREE.Float32BufferAttribute(nor, 3));
      g.setIndex(idx);
      add('Face9', 'f0000001.fillet.Face(G p0000001.pad.Edge(G s0000001.na000003, G s0000001.na000004))', 'f0000001', 'cylinder',
        { geom: g, frame: null }, { radius: r, axis: [0, 1, 0] });
    }
    let fi = r > 0 ? 10 : 9;
    const cyl = (c, axis, len, name, stable, maker) => {
      const g = new THREE.CylinderGeometry(hr, hr, len, 40, 1, true);
      if (axis === 'z') g.rotateX(Math.PI / 2); else g.rotateZ(Math.PI / 2);
      g.translate(c.x, c.y, c.z);
      faces.push({ id: name, stable, maker, kind: 'cylinder', geom: g, frame: null, area: 2 * Math.PI * hr * len, radius: hr, axis: axis === 'z' ? [0, 0, 1] : [1, 0, 0] });
    };
    baseHoles.forEach((h, i) => cyl(V3(h[0], h[1], tb / 2), 'z', tb, 'Face' + fi++, `h0000001.pocket.Face(G s0000002.cb00000${i + 1})`, 'h0000001'));
    upHoles.forEach((h, i) => cyl(V3(tu / 2, h[0], h[1]), 'x', tu, 'Face' + fi++, `h0000002.pocket.Face(G s0000003.cc00000${i + 1})`, 'h0000002'));

    // Edges: the profile's curves at both caps, the lengthwise edges, and the holes' circles.
    let ei = 1;
    const addEdge = (pts, kind, maker, stable, extra) => edges.push(Object.assign({ id: 'Edge' + ei++, pts, kind, maker, stable, length: polyLength(pts) }, extra || {}));
    const corners = [[0, 0], [W, 0], [W, tb]].concat(r > 0 ? [[tu + r, tb], [tu, tb + r]] : [[tu, tb]]).concat([[tu, H], [0, H]]);
    const lineIds = ['na000001', 'na000002', 'na000003', 'na000004', 'na000005', 'na000006'];
    for (const y of [0, D]) {
      const cap = y === 0 ? 'Start' : 'End';
      let li = 0;
      for (let i = 0; i < corners.length; i++) {
        const a = corners[i], b = corners[(i + 1) % corners.length];
        const isArc = r > 0 && i === 3;
        if (isArc) {
          const pts = [];
          for (let k = 0; k <= 16; k++) { const t = -Math.PI / 2 - (k / 16) * (Math.PI / 2); pts.push(V3(tu + r + r * Math.cos(t), y, tb + r + r * Math.sin(t))); }
          addEdge(pts, 'arc', 'f0000001', `f0000001.fillet.Edge(${cap})`, { radius: r });
        } else {
          addEdge([V3(a[0], y, a[1]), V3(b[0], y, b[1])], 'line', 'p0000001', `p0000001.pad.Edge(${cap}, G s0000001.${lineIds[li]})`);
          li++;
        }
      }
    }
    // Which profile lines meet at each corner (a fillet's two tangent edges meet one line each).
    const meets = r > 0 ? [[6, 1], [1, 2], [2, 3], [3], [4], [4, 5], [5, 6]] : [[6, 1], [1, 2], [2, 3], [3, 4], [4, 5], [5, 6]];
    corners.forEach((c, i) => {
      const m = meets[i];
      const filletSide = m.length === 1;
      addEdge([V3(c[0], 0, c[1]), V3(c[0], D, c[1])], 'line', filletSide ? 'f0000001' : 'p0000001',
        filletSide ? `f0000001.fillet.Edge(G p0000001.pad.Face(G s0000001.na00000${m[0]}))` : `p0000001.pad.Edge(G s0000001.na00000${m[0]}, G s0000001.na00000${m[1]})`,
        { lengthwise: true, corner: c });
    });
    baseHoles.forEach((h, i) => {
      for (const z of [0, tb]) addEdge(circlePts(V3(h[0], h[1], z), X, Y, hr), 'circle', 'h0000001', `h0000001.pocket.Edge(G s0000002.cb00000${i + 1}, ${z ? 'Top' : 'Bottom'})`, { radius: hr });
    });
    upHoles.forEach((h, i) => {
      for (const x of [0, tu]) addEdge(circlePts(V3(x, h[0], h[1]), Y, Z, hr), 'circle', 'h0000002', `h0000002.pocket.Edge(G s0000003.cc00000${i + 1}, ${x ? 'Inner' : 'Outer'})`, { radius: hr });
    });
    let vi = 1;
    for (const y of [0, D]) for (const c of corners) verts.push({ id: 'Vertex' + vi++, p: V3(c[0], y, c[1]), maker: 'p0000001' });

    const vol = (W * tb + (H - tb) * tu) * D + (r > 0 ? r * r * (1 - Math.PI / 4) * D : 0)
      - Math.PI * hr * hr * (tb * baseHoles.length + tu * upHoles.length);
    const totalArea = faces.reduce((s, f) => s + f.area, 0);
    return { faces, edges, verts, volume: vol, area: totalArea, bbox: [W, D, H], hasFillet: r > 0, holes: baseHoles.length + upHoles.length };
  }

  // Simple solids for the start page's thumbnails.
  function plateGeometry() {
    const s = rectShape(0, 0, 80, 50, [[10, 10, 3], [70, 10, 3], [10, 40, 3], [70, 40, 3], [40, 25, 9]]);
    return new THREE.ExtrudeGeometry(s, { depth: 6, bevelEnabled: false, curveSegments: 32 });
  }
  function chamferBoxGeometry() {
    const s = rectShape(2, 2, 38, 38);
    return new THREE.ExtrudeGeometry(s, { depth: 26, bevelEnabled: true, bevelThickness: 2, bevelSize: 2, bevelSegments: 1 });
  }

  // ───────── The plate of docs/design/m6.0-patterns.md section 2.2, face by face ─────────
  // g: what the stand-in core built (patterns.js's solve), in timeline order:
  //   rects:   [{x0, x1, y0, y1, maker, names: {x0, x1, y0, y1}}]  through-all cuts (notches)
  //   circles: [{cx, cy, r, maker, stable}]                        through-all cuts (holes)
  //   bosses:  [{cx, cy, r, z0, z1, maker, side, top}]              fused cylinders on the top face
  // The plate itself is 100 × 60 × 5, centred on the origin, as Plate outline and Plate give it.
  // Prototype limit: a hole across the plate's edge is cut by the core but not drawn here.
  function buildPlate(g) {
    const T = 5, X0 = -50, X1 = 50, Y0 = -30, Y1 = 30;
    const X = V3(1, 0, 0), Y = V3(0, 1, 0), Z = V3(0, 0, 1);
    const TOP = 'p0000001.pad.Face(G s0000001.sketch.Face1)', BOTTOM = 'p0000001.pad.Face(M s0000001.sketch.Face1)';
    const PLATE_SIDE = { x0: 'na000004', x1: 'na000002', y0: 'na000001', y1: 'na000003' };
    const rects = g.rects || [], circles = g.circles || [], bosses = g.bosses || [];
    const inPlate = (x, y) => x > X0 && x < X1 && y > Y0 && y < Y1;
    const cutBy = (x, y) => rects.find((r) => x > r.x0 && x < r.x1 && y > r.y0 && y < r.y1);
    const filledAt = (x, y) => inPlate(x, y) && !cutBy(x, y);
    // The region: the plate's rectangle less the notches, on the grid of every x and y they use.
    const xs = [...new Set([X0, X1, ...rects.flatMap((r) => [r.x0, r.x1])].map((v) => Math.max(X0, Math.min(X1, v))))].sort((a, b) => a - b);
    const ys = [...new Set([Y0, Y1, ...rects.flatMap((r) => [r.y0, r.y1])].map((v) => Math.max(Y0, Math.min(Y1, v))))].sort((a, b) => a - b);
    const fill = (i, j) => i >= 0 && j >= 0 && i < xs.length - 1 && j < ys.length - 1 && filledAt((xs[i] + xs[i + 1]) / 2, (ys[j] + ys[j + 1]) / 2);
    const tagOf = (nx, ny, side) => {
      // What's beyond a boundary: outside the plate (one of its sides) or inside a notch (one of the notch's walls).
      if (!inPlate(nx, ny)) return { maker: 'p0000001', stable: `p0000001.pad.Face(G s0000001.sketch.Edge.${PLATE_SIDE[side]})` };
      const r = cutBy(nx, ny);
      const opp = { x0: 'x1', x1: 'x0', y0: 'y1', y1: 'y0' }[side];
      return { maker: r.maker, stable: r.names[opp] };
    };
    const segs = [];
    for (let i = 0; i < xs.length - 1; i++) for (let j = 0; j < ys.length - 1; j++) {
      if (!fill(i, j)) continue;
      const a = xs[i], b = xs[i + 1], c = ys[j], d = ys[j + 1], mx = (a + b) / 2, my = (c + d) / 2;
      if (!fill(i, j - 1)) segs.push({ p: [a, c], q: [b, c], tag: tagOf(mx, c - 1e-6, 'y0') });
      if (!fill(i + 1, j)) segs.push({ p: [b, c], q: [b, d], tag: tagOf(b + 1e-6, my, 'x1') });
      if (!fill(i, j + 1)) segs.push({ p: [b, d], q: [a, d], tag: tagOf(mx, d + 1e-6, 'y1') });
      if (!fill(i - 1, j)) segs.push({ p: [a, d], q: [a, c], tag: tagOf(a - 1e-6, my, 'x0') });
    }
    const k = (p) => p[0].toFixed(6) + ',' + p[1].toFixed(6);
    const from = new Map(); segs.forEach((s) => { const key = k(s.p); (from.get(key) || from.set(key, []).get(key)).push(s); });
    const used = new Set(); const loops = [];
    for (const s0 of segs) {
      if (used.has(s0)) continue;
      const loop = []; let s = s0;
      while (s && !used.has(s)) { used.add(s); loop.push(s); s = (from.get(k(s.q)) || []).find((x) => !used.has(x)); }
      // Merge runs along one line with one maker into a single wall.
      const merged = [];
      for (const x of loop) {
        const L = merged[merged.length - 1];
        const col = L && L.tag.stable === x.tag.stable && Math.abs((L.q[0] - L.p[0]) * (x.q[1] - x.p[1]) - (L.q[1] - L.p[1]) * (x.q[0] - x.p[0])) < 1e-9;
        if (col) L.q = x.q; else merged.push({ p: x.p, q: x.q, tag: x.tag });
      }
      if (merged.length > 1) { const F = merged[0], L = merged[merged.length - 1]; if (F.tag.stable === L.tag.stable && Math.abs((L.q[0] - L.p[0]) * (F.q[1] - F.p[1]) - (L.q[1] - L.p[1]) * (F.q[0] - F.p[0])) < 1e-9) { F.p = L.p; merged.pop(); } }
      let ar = 0; merged.forEach((m) => { ar += m.p[0] * m.q[1] - m.q[0] * m.p[1]; });
      loops.push({ segs: merged, area: ar / 2 });
    }
    const outers = loops.filter((l) => l.area > 0), inners = loops.filter((l) => l.area < 0);
    const ptIn = (pt, l) => { let c = false; for (const s of l.segs) { const [x1, y1] = s.p, [x2, y2] = s.q; if ((y1 > pt[1]) !== (y2 > pt[1]) && pt[0] < ((x2 - x1) * (pt[1] - y1)) / (y2 - y1) + x1) c = !c; } return c; };
    const regionHolds = (cx, cy, r) => { for (let i = 0; i < 24; i++) { const t = (i / 24) * Math.PI * 2; if (!filledAt(cx + Math.cos(t) * (r + 0.01), cy + Math.sin(t) * (r + 0.01))) return false; } return filledAt(cx, cy); };
    const inDisk = (c, b) => Math.hypot(c.cx - b.cx, c.cy - b.cy) + c.r <= b.r + 1e-9;
    const drawnBosses = bosses.filter((b) => regionHolds(b.cx, b.cy, b.r));
    const drawnHoles = circles.filter((c) => regionHolds(c.cx, c.cy, c.r));

    const faces = [], edges = [], verts = [];
    let fi = 1, ei = 1, vi = 1;
    const addFace = (stable, maker, kind, built, extra) => { const f = Object.assign({ id: 'Face' + fi++, stable, maker, kind, geom: built.geom, frame: built.frame, area: area(built.geom) }, extra || {}); faces.push(f); return f; };
    const prefix = (stable) => stable.slice(0, stable.indexOf('.Face('));
    const addEdge = (pts, kind, maker, stable, extra) => { const e = Object.assign({ id: 'Edge' + ei++, pts, kind, maker, stable, length: polyLength(pts) }, extra || {}); edges.push(e); return e; };
    const holePath = (c) => { const p = new THREE.Path(); p.absarc(c.cx, c.cy, c.r, 0, Math.PI * 2, true); return p; };
    const loopShape = (l, reverse) => { const pts = l.segs.map((s) => new THREE.Vector2(s.p[0], s.p[1])); if (reverse) pts.reverse(); return pts; };
    for (const o of outers) {
      for (const [z, stable, top] of [[T, TOP, true], [0, BOTTOM, false]]) {
        const sh = new THREE.Shape(loopShape(o));
        for (const h of inners.filter((h) => ptIn(h.segs[0].p, o))) sh.holes.push(new THREE.Path(loopShape(h, true)));
        for (const c of drawnHoles) if (ptIn([c.cx, c.cy], o) && !(top && drawnBosses.some((b) => inDisk(c, b)))) sh.holes.push(holePath(c));
        if (top) for (const b of drawnBosses) if (ptIn([b.cx, b.cy], o)) sh.holes.push(holePath(b));
        addFace(stable, 'p0000001', 'plane', planeFace(sh, V3(0, 0, z), X, Y), { normal: [0, 0, top ? 1 : -1] });
      }
    }
    for (const l of loops) {
      const walls = l.segs.map((s) => {
        const a = V3(s.p[0], s.p[1], 0), b = V3(s.q[0], s.q[1], 0); const len = a.distanceTo(b); const U = b.clone().sub(a).normalize();
        const w = addFace(s.tag.stable, s.tag.maker, 'plane', planeFace(rectShape(0, 0, len, T), a, U, Z), { normal: [U.y, -U.x, 0].map((v) => Math.round(v * 1e9) / 1e9 + 0) });
        for (const [z, cap] of [[T, TOP], [0, BOTTOM]]) addEdge([V3(s.p[0], s.p[1], z), V3(s.q[0], s.q[1], z)], 'line', s.tag.maker, `${prefix(s.tag.stable)}.Edge(G ${cap}, G ${s.tag.stable})`);
        return w;
      });
      l.segs.forEach((s, i) => {
        const prev = walls[(i - 1 + walls.length) % walls.length], cur = walls[i];
        addEdge([V3(s.p[0], s.p[1], 0), V3(s.p[0], s.p[1], T)], 'line', cur.maker, `${prefix(cur.stable)}.Edge(G ${prev.stable}, G ${cur.stable})`);
        for (const z of [0, T]) verts.push({ id: 'Vertex' + vi++, p: V3(s.p[0], s.p[1], z), maker: cur.maker });
      });
    }
    const cylinder = (c, z0, z1, stable, maker) => {
      const geo = new THREE.CylinderGeometry(c.r, c.r, z1 - z0, 48, 1, true); geo.rotateX(Math.PI / 2); geo.translate(c.cx, c.cy, (z0 + z1) / 2);
      return addFace(stable, maker, 'cylinder', { geom: geo, frame: null }, { radius: c.r, axis: [0, 0, 1], center: [c.cx, c.cy] });
    };
    for (const b of drawnBosses) {
      const side = cylinder(b, b.z0, b.z1, b.side, b.maker);
      const sh = new THREE.Shape(); sh.absarc(b.cx, b.cy, b.r, 0, Math.PI * 2, false);
      for (const c of drawnHoles) if (inDisk(c, b)) sh.holes.push(holePath(c));
      addFace(b.top, b.maker, 'plane', planeFace(sh, V3(0, 0, b.z1), X, Y), { normal: [0, 0, 1] });
      for (const [z, cap] of [[b.z0, TOP], [b.z1, b.top]]) addEdge(circlePts(V3(b.cx, b.cy, z), X, Y, b.r, 64), 'circle', b.maker, `${prefix(b.side)}.Edge(G ${cap}, G ${side.stable})`, { radius: b.r, center: [b.cx, b.cy, z] });
    }
    for (const c of drawnHoles) {
      const boss = drawnBosses.find((b) => inDisk(c, b));
      const z1 = boss ? boss.z1 : T;
      const w = cylinder(c, 0, z1, c.stable, c.maker);
      for (const [z, cap] of [[0, BOTTOM], [z1, boss ? boss.top : TOP]]) addEdge(circlePts(V3(c.cx, c.cy, z), X, Y, c.r), 'circle', c.maker, `${prefix(c.stable)}.Edge(G ${cap}, G ${w.stable})`, { radius: c.r, center: [c.cx, c.cy, z] });
    }
    const regionArea = outers.reduce((s, l) => s + l.area, 0) + inners.reduce((s, l) => s + l.area, 0);
    let vol = regionArea * T;
    for (const b of drawnBosses) vol += Math.PI * b.r * b.r * (b.z1 - b.z0);
    for (const c of drawnHoles) { const boss = drawnBosses.find((b) => inDisk(c, b)); vol -= Math.PI * c.r * c.r * (boss ? boss.z1 : T); }
    const totalArea = faces.reduce((s, f) => s + f.area, 0);
    const zTop = Math.max(T, ...drawnBosses.map((b) => b.z1));
    return { faces, edges, verts, volume: vol, area: totalArea, bbox: [X1 - X0, Y1 - Y0, zTop], hasFillet: false, holes: drawnHoles.length, undrawn: circles.length - drawnHoles.length };
  }

  window.Geo = { buildBracket, buildPlate, plateGeometry, chamferBoxGeometry, rectShape };
})();
