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

  window.Geo = { buildBracket, plateGeometry, chamferBoxGeometry, rectShape };
})();
