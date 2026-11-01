/* Sketch mode: drawn over the viewport by the app (egui's Painter over the
   re_renderer callback), from what the `sketch` read gives: each member's
   definition state, the profile, constraints, and dimensions. */
(function () {
  const T = THREE;
  const A = () => window.APP;
  const COL = { full: '#eeeeed', under: '#7a95ff', over: '#ff3865', invalid: '#f2c94c', dangling: '#b5b84a', driven: '#827f7f', sel: '#4676ff' };
  const GLYPH = { H: 'cHorizontal', V: 'cVertical', coinc: 'cCoincident', fix: 'cFix' };
  const toV = (a) => (a.isVector3 ? a.clone() : new T.Vector3(a[0] ?? a.x, a[1] ?? a.y, a[2] ?? a.z));
  const FRAMES = {
    XY: { o: [0, 0, 0], U: [1, 0, 0], V: [0, 1, 0], view: { az: 0, el: Math.PI / 2 }, uL: 'x', vL: 'y', uC: '#ff6981', vC: '#00b266' },
    XZ: { o: [0, 0, 0], U: [1, 0, 0], V: [0, 0, 1], view: { az: 0, el: 0 }, uL: 'x', vL: 'z', uC: '#ff6981', vC: '#7a95ff' },
    YZ: { o: [0, 0, 0], U: [0, 1, 0], V: [0, 0, 1], view: { az: Math.PI / 2, el: 0 }, uL: 'y', vL: 'z', uC: '#00b266', vC: '#7a95ff' },
  };
  let K = null; // the open sketch's state (S.sketch)

  function frameOf(K) { return { o: toV(K.frame.o), U: toV(K.frame.U), V: toV(K.frame.V) }; }
  function w2s(u, v) { const f = frameOf(K); return VP.project(f.o.clone().addScaledVector(f.U, u).addScaledVector(f.V, v)); }

  // ── Bracket profile: the L, its constraints and dimensions ──
  function lpoints() {
    const { W, H, tb } = K.dims; const tu = K.tu;
    return { P1: [0, 0], P2: [W, 0], P3: [W, tb], P4: [tu, tb], P5: [tu, H], P6: [0, H] };
  }
  const LLINES = [['na000001', 'P1', 'P2', 'H'], ['na000002', 'P2', 'P3', 'V'], ['na000003', 'P3', 'P4', 'H'], ['na000004', 'P4', 'P5', 'V'], ['na000005', 'P5', 'P6', 'H'], ['na000006', 'P6', 'P1', 'V']];
  function lstate() {
    const tuDefined = K.placed.tu;
    const over = K.extra.some((x) => !x.driven && x.conflict);
    const under = new Set(tuDefined ? [] : ['na000003', 'na000004', 'na000005', 'P4', 'P5']);
    const def = over ? 'over' : under.size ? 'under' : 'full';
    return { def, dof: tuDefined ? 0 : 1, under, over };
  }

  function state() {
    if (!K) return { def: 'full', dof: 0 };
    if (K.kind === 'bracket') return lstate();
    const pts = Object.values(K.points).filter((p) => !p.origin);
    const rel = K.lines.filter((l) => l.rel).length;
    const dof = Math.max(0, pts.length * 2 - rel);
    return { def: K.lines.length || pts.length ? (dof ? 'under' : 'full') : 'empty', dof };
  }

  // ── Enter, finish, cancel ──
  function enter(opts) {
    const S = A().S; const d = A().doc();
    S.sel = []; S.hover = null; S.selFeature = null; S.dialog = null; S.pickPlane = null;
    const fr = opts.frame || FRAMES[opts.plane];
    K = {
      kind: opts.featureId === 's0000001' ? 'bracket' : 'new',
      featureId: opts.featureId || null, plane: opts.plane || 'Face', name: opts.name,
      frame: { o: fr.o, U: fr.U, V: fr.V }, uL: fr.uL || 'u', vL: fr.vL || 'v', uC: fr.uC || '#ff6981', vC: fr.vC || '#7a95ff',
      tool: null, chain: null, cursor: null, inf: null, hoverLine: null, modify: null,
      opts: { constraints: true, dims: true, points: true, profile: true, auto: true },
      prevCam: { az: VP.cam.az, el: VP.cam.el, target: VP.cam.target.clone(), height: VP.cam.height },
      openSnap: A().snap(d), rollModel: opts.featureId === 's0000001',
      points: { origin: { u: 0, v: 0, origin: true } }, lines: [], seq: 1,
    };
    if (K.kind === 'bracket') {
      K.dims = { W: d.p.W, H: d.p.H, tb: d.p.tb }; K.tu = d.p.tu;
      K.placed = { tu: opts.mode !== 'placing' };
      K.extra = [];
      if (opts.mode === 'placing') { K.tool = 'dimension'; K.modify = { target: 'tu', text: String(d.p.tu), line: 'na000005', fresh: true }; }
    }
    S.sketch = K;
    window.APP.S.sketch = K;
    if (K.rollModel) VP.setModel(null); else VP.setDim(0.45);
    VP.setGridVisible(false);
    VP.setSketchLines([]);
    // Look at the sketch plane.
    const view = fr.view || viewFromNormal(frameOf(K));
    const box = new T.Box3();
    if (K.kind === 'bracket') { box.expandByPoint(new T.Vector3(-4, 0, -4)); box.expandByPoint(new T.Vector3(K.dims.W + 4, 0, K.dims.H + 4)); }
    else { const f = frameOf(K); const c = f.o; box.expandByPoint(c.clone().addScaledVector(f.U, -20).addScaledVector(f.V, -20)); box.expandByPoint(c.clone().addScaledVector(f.U, 80).addScaledVector(f.V, 60)); }
    VP.fitBox(box, view);
    A().render();
    if (K.modify) setTimeout(focusModify, 260);
  }
  function viewFromNormal(f) {
    const N = new T.Vector3().crossVectors(f.U, f.V).normalize();
    return { az: Math.abs(N.z) > 0.999 ? 0 : Math.atan2(N.x, -N.y), el: Math.asin(Math.max(-1, Math.min(1, N.z))) };
  }
  function exit() {
    const S = A().S; const c = K.prevCam;
    K = null; S.sketch = null;
    VP.setDim(1); VP.setGridVisible(S.grid !== false);
    A().publish(A().doc());
    VP.animateTo(c);
    A().render();
  }
  function finish() {
    if (!K) return;
    const k = K; const d = A().doc();
    if (k.modify) { toastA('Accept or cancel the value first (Enter or Esc)'); return; }
    if (k.kind === 'bracket') {
      const st = lstate();
      const over = st.def === 'over';
      exit();
      A().edit((dd) => { dd.p.tu = k.tu; dd.sketchState.s0000001 = over ? 'over' : st.def === 'under' ? 'under' : 'full'; }, { label: 'Edit Bracket profile' });
      if (!(k.tu !== d.p.tu || over)) A().toast('Bracket profile unchanged');
      return;
    }
    if (!k.lines.length) { exit(); A().toast('Empty sketch discarded. Nothing was added.'); return; }
    const n = d.features.filter((f) => f.type === 'sketch').length + 1;
    const id = 's' + String(n).padStart(7, '0') + (d.kind === 'bracket' ? 'n' : '');
    const name = 'Sketch' + n;
    exit();
    A().edit((dd) => {
      dd.newSketches[id] = { name, plane: k.plane, frame: { o: toV(k.frame.o).toArray(), U: toV(k.frame.U).toArray(), V: toV(k.frame.V).toArray() }, points: k.points, lines: k.lines };
      dd.features.push({ id, type: 'sketch', name, plane: k.plane, inputs: [] });
      dd.sketchState[id] = 'under';
    }, { label: 'Add ' + name });
    A().S.vis.sketches[id] = true; A().syncSketchLines(); A().render();
    A().toast(name + ' added. Pad (in CREATE) makes it solid once its profile closes.', 'check');
  }
  function cancel() { if (!K) return; exit(); A().toast('Sketch changes discarded'); }
  function toastA(t) { A().toast(t); }

  // ── Modify box (Smart Dimension's value) ──
  function focusModify() { const el = document.querySelector('[data-key="sk-mod"]'); if (el) { el.focus(); el.select(); } }
  function modifyInput(v) { if (K && K.modify) { K.modify.text = v; K.modify.err = null; } }
  function modifyAccept() {
    const M = K.modify; const r = A().parseValue(M.text, 'A length');
    if (r.err) { M.err = r.err; A().render(); return; }
    if (M.target === 'tu') { K.tu = r.v; K.placed.tu = true; K.modify = null; A().render(); A().toast('Dimension placed. Bracket profile is fully defined.', 'check'); return; }
    if (M.target === 'line') {
      const st = lstate();
      if (!st.under.has(M.line)) {
        K.pendingExtra = { id: 'ka00000d', line: M.line, value: r.v };
        K.modify = null;
        A().S.modal = { type: 'overdefine', sketch: 'Bracket profile', value: A().fmt(r.v) };
        A().render(); return;
      }
      // Dimensioning a free line defines the upright's thickness.
      if (M.line === 'na000005') K.tu = r.v; else if (M.line === 'na000003') K.tu = K.dims.W - r.v; else K.tu = K.tu;
      K.placed.tu = true; K.modify = null; A().render(); A().toast('Bracket profile is fully defined.', 'check');
    }
  }
  function overdefineChoice(v) {
    const S = A().S; S.modal = null;
    const p = K.pendingExtra; K.pendingExtra = null;
    if (v === 'driven') K.extra.push({ ...p, driven: true });
    if (v === 'driving') K.extra.push({ ...p, driven: false, conflict: true });
    A().render();
    if (v === 'driving') A().toast('Bracket profile is over defined. Finish keeps it, and Legs then fails.', 'warn', 'err');
  }

  // ── Pointer ──
  function sketchPt(e) {
    const f = frameOf(K); const h = VP.planeHit(e.clientX, e.clientY, f); if (!h) return null;
    return { u: h.u, v: h.v };
  }
  function nearestLine(e) {
    if (K.kind !== 'bracket') return null;
    const r = document.getElementById('gl').getBoundingClientRect(); const m = { x: e.clientX - r.left, y: e.clientY - r.top };
    const P = lpoints(); let best = null;
    for (const [id, a, b] of LLINES) {
      const A2 = w2s(...P[a]), B2 = w2s(...P[b]);
      const dx = B2.x - A2.x, dy = B2.y - A2.y, L = dx * dx + dy * dy; let t = ((m.x - A2.x) * dx + (m.y - A2.y) * dy) / L; t = Math.max(0, Math.min(1, t));
      const dd = Math.hypot(m.x - A2.x - t * dx, m.y - A2.y - t * dy);
      if (dd < 7 && (!best || dd < best.d)) best = { id, d: dd };
    }
    return best && best.id;
  }
  function onMove(e) {
    if (!K) return;
    const p = sketchPt(e); K.cursor = p;
    if (K.tool === 'dimension' && !K.modify) K.hoverLine = nearestLine(e);
    if (K.tool === 'line' && p) infer(p, e);
    A().renderStatus(); VP.request();
  }
  function infer(p, e) {
    const ppm = VP.pxPerUnit(); const tolU = 8 / ppm;
    K.inf = { snap: null, rel: null, align: null };
    let best = null;
    for (const [id, q] of Object.entries(K.points)) { const d = Math.hypot(q.u - p.u, q.v - p.v); if (d < tolU && (!best || d < best.d)) best = { id, d }; }
    if (best) { K.inf.snap = best.id; p.u = K.points[best.id].u; p.v = K.points[best.id].v; return; }
    if (K.chain && !(e && (e.ctrlKey || e.metaKey)) && K.opts.auto) {
      const a = K.points[K.chain.from]; const ang = Math.atan2(p.v - a.v, p.u - a.u) * 180 / Math.PI;
      const near = (x) => Math.abs(((ang - x) % 360 + 540) % 360 - 180) < 3.5;
      if (near(0) || near(180)) { p.v = a.v; K.inf.rel = 'H'; }
      else if (near(90) || near(-90)) { p.u = a.u; K.inf.rel = 'V'; }
    }
    // Dotted guides to woken points (lines up, adds nothing for a line's end).
    for (const [id, q] of Object.entries(K.points)) {
      if (K.chain && id === K.chain.from) continue;
      if (Math.abs(q.u - p.u) < tolU * 0.6) { p.u = q.u; K.inf.align = { id, axis: 'u' }; break; }
      if (Math.abs(q.v - p.v) < tolU * 0.6) { p.v = q.v; K.inf.align = { id, axis: 'v' }; break; }
    }
  }
  function onClick(e) {
    if (!K) return;
    if (K.tool === 'dimension' && !K.modify) {
      const id = nearestLine(e);
      if (id) {
        const P = lpoints(); const l = LLINES.find((x) => x[0] === id); const len = Math.hypot(P[l[2]][0] - P[l[1]][0], P[l[2]][1] - P[l[1]][1]);
        K.modify = { target: 'line', line: id, text: A().fmt(len, 3), fresh: true };
        A().render(); setTimeout(focusModify, 0);
      } else A().toast('Smart Dimension: click a line. Esc leaves the tool.');
      return;
    }
    if (K.tool === 'line') {
      onMove(e);
      const p = K.cursor; if (!p) return;
      let pid = K.inf && K.inf.snap;
      const existed = !!pid;
      if (!pid) { pid = 'pt' + K.seq++; K.points[pid] = { u: p.u, v: p.v }; }
      if (!K.chain) { K.chain = { from: pid }; }
      else {
        if (pid === K.chain.from) return;
        K.lines.push({ id: 'ln' + K.seq++, a: K.chain.from, b: pid, rel: K.inf && K.inf.rel });
        if (existed && pid !== 'origin') K.chain = null; else K.chain = { from: pid };
        if (existed && pid === 'origin') K.chain = null;
      }
      A().render();
      return;
    }
    if (!K.tool) A().toast('Pick a tool: Line (L), Smart Dimension (D)…');
  }
  function onDbl() { if (K && K.chain) { K.chain = null; A().render(); } }
  function setTool(t) {
    if (!K) return;
    if (['rect', 'circle', 'arc', 'point', 'trim'].includes(t) && K.kind === 'bracket') { A().toast('This prototype draws lines (L) in a new sketch, and places dimensions (D).'); }
    if (t === 'line' && K.kind === 'bracket') { A().toast('Lines are drawn in a new sketch in this prototype: Create Sketch, then pick a plane.'); return; }
    K.tool = K.tool === t ? null : t; K.chain = null; K.inf = null; K.modify = null;
    document.getElementById('canvas-wrap').classList.toggle('tool-line', K.tool === 'line');
    A().render();
  }
  // Esc: the nearest thing first.
  function esc() {
    if (!K) return false;
    if (K.modify) { K.modify = null; A().render(); return true; }
    if (K.chain) { K.chain = null; A().render(); return true; }
    if (K.tool) { setTool(K.tool); return true; }
    return false;
  }

  // ── Drawing ──
  function overlay(w, h) {
    if (!K) return '';
    let s = '';
    const f = frameOf(K);
    // Grid on the sketch plane, every 5 mm (10 when zoomed out).
    const ppm = VP.pxPerUnit(); const step = ppm > 6 ? 5 : ppm > 2 ? 10 : 50;
    const rect = document.getElementById('gl').getBoundingClientRect();
    const c0 = VP.planeHit(rect.left, rect.top, f), c1 = VP.planeHit(rect.right, rect.bottom, f);
    if (c0 && c1) {
      const u0 = Math.floor(Math.min(c0.u, c1.u) / step) * step, u1 = Math.max(c0.u, c1.u), v0 = Math.floor(Math.min(c0.v, c1.v) / step) * step, v1 = Math.max(c0.v, c1.v);
      for (let u = u0; u <= u1; u += step) { const a = w2s(u, v0), b = w2s(u, v1); s += `<line x1="${a.x}" y1="${a.y}" x2="${b.x}" y2="${b.y}" stroke="${u % 50 === 0 ? '#2c2b2b' : '#171717'}" stroke-width="1"/>`; }
      for (let v = v0; v <= v1; v += step) { const a = w2s(u0, v), b = w2s(u1, v); s += `<line x1="${a.x}" y1="${a.y}" x2="${b.x}" y2="${b.y}" stroke="${v % 50 === 0 ? '#2c2b2b' : '#171717'}" stroke-width="1"/>`; }
      const ua = w2s(u0, 0), ub = w2s(u1, 0), va = w2s(0, v0), vb = w2s(0, v1);
      s += `<line x1="${ua.x}" y1="${ua.y}" x2="${ub.x}" y2="${ub.y}" stroke="${K.uC}" stroke-opacity=".55" stroke-width="1"/>`;
      s += `<line x1="${va.x}" y1="${va.y}" x2="${vb.x}" y2="${vb.y}" stroke="${K.vC}" stroke-opacity=".55" stroke-width="1"/>`;
    }
    const o = w2s(0, 0); s += `<circle cx="${o.x}" cy="${o.y}" r="3" fill="none" stroke="#cfcfcf" stroke-width="1.2"/>`;
    if (K.kind === 'bracket') s += drawBracket(); else s += drawNew();
    return s;
  }
  function seg(a, b, col, wdt, dash) { return `<line x1="${a.x.toFixed(1)}" y1="${a.y.toFixed(1)}" x2="${b.x.toFixed(1)}" y2="${b.y.toFixed(1)}" stroke="${col}" stroke-width="${wdt}" stroke-linecap="round" ${dash ? `stroke-dasharray="${dash}"` : ''}/>`; }
  function glyph(x, y, name, col) {
    return `<g transform="translate(${(x - 8).toFixed(1)},${(y - 8).toFixed(1)})"><rect width="16" height="16" rx="3" fill="#212121" stroke="${col || '#525151'}"/><g transform="translate(2,2) scale(0.6)" stroke="${col || '#bfbfbf'}" fill="none" stroke-width="2.2" stroke-linecap="round">${ICONS[name]}</g></g>`;
  }
  function dimension(a, b, nx, ny, off, label, col, opts = {}) {
    // a, b: screen points; (nx, ny): unit offset direction in screen space
    const A1 = { x: a.x + nx * off, y: a.y + ny * off }, B1 = { x: b.x + nx * off, y: b.y + ny * off };
    let s = seg({ x: a.x + nx * 4, y: a.y + ny * 4 }, { x: A1.x + nx * 5, y: A1.y + ny * 5 }, col, 1) + seg({ x: b.x + nx * 4, y: b.y + ny * 4 }, { x: B1.x + nx * 5, y: B1.y + ny * 5 }, col, 1);
    s += seg(A1, B1, col, 1);
    const dx = B1.x - A1.x, dy = B1.y - A1.y, L = Math.hypot(dx, dy) || 1, ux = dx / L, uy = dy / L;
    const arrow = (P, sx) => `<path d="M${P.x},${P.y} l${(sx * ux * 7 - uy * 3).toFixed(1)},${(sx * uy * 7 + ux * 3).toFixed(1)} l${(uy * 6).toFixed(1)},${(-ux * 6).toFixed(1)} z" fill="${col}"/>`;
    s += arrow(A1, 1) + arrow(B1, -1);
    const mx = (A1.x + B1.x) / 2, my = (A1.y + B1.y) / 2;
    if (!opts.noLabel) {
      const tw = label.length * 6.6 + 10;
      s += `<rect x="${(mx - tw / 2).toFixed(1)}" y="${(my - 9).toFixed(1)}" width="${tw}" height="18" rx="3" fill="#000" fill-opacity=".85" ${opts.box ? `stroke="${col}"` : ''}/>`;
      s += `<text x="${mx.toFixed(1)}" y="${(my + 4).toFixed(1)}" fill="${col}" font-size="11.5" text-anchor="middle" font-family="Inter Silico, Inter" style="font-variant-numeric:tabular-nums">${label}</text>`;
    }
    return { svg: s, mid: { x: mx, y: my } };
  }
  function drawBracket() {
    const P = lpoints(); const st = lstate(); let s = '';
    const S = Object.fromEntries(Object.entries(P).map(([k, p]) => [k, w2s(...p)]));
    const colOf = (id) => {
      const x = K.extra.find((e) => e.line === id && !e.driven && e.conflict);
      if (x || (st.over && ['na000001', 'na000003', 'na000005'].includes(id))) return COL.over;
      return st.under.has(id) ? COL.under : COL.full;
    };
    if (K.opts.profile && st.def !== 'over') {
      s += `<polygon points="${['P1', 'P2', 'P3', 'P4', 'P5', 'P6'].map((k) => S[k].x.toFixed(1) + ',' + S[k].y.toFixed(1)).join(' ')}" fill="#186add" fill-opacity=".1"/>`;
    }
    for (const [id, a, b] of LLINES) {
      const hov = K.hoverLine === id || (K.modify && K.modify.line === id);
      if (hov) s += seg(S[a], S[b], COL.sel, 6, null).replace('stroke-linecap', 'stroke-opacity=".35" stroke-linecap');
      s += seg(S[a], S[b], hov ? '#ffffff' : colOf(id), hov ? 2.4 : 1.8);
    }
    if (K.opts.points) for (const [k, p] of Object.entries(S)) {
      const c = st.under.has(k) ? COL.under : st.def === 'over' ? COL.over : COL.full;
      s += `<rect x="${(p.x - 2.5).toFixed(1)}" y="${(p.y - 2.5).toFixed(1)}" width="5" height="5" fill="${c}"/>`;
    }
    if (K.opts.constraints) {
      for (const [id, a, b, rel] of LLINES) {
        const A1 = S[a], B1 = S[b]; const dx = B1.x - A1.x, dy = B1.y - A1.y, L = Math.hypot(dx, dy) || 1;
        const t = 0.28; const nx = -dy / L, ny = dx / L;
        s += glyph(A1.x + dx * t + nx * 13, A1.y + dy * t + ny * 13, GLYPH[rel]);
      }
      s += glyph(S.P1.x - 14, S.P1.y + 14, 'cCoincident');
    }
    if (K.opts.dims) {
      const { W, H, tb } = K.dims;
      const dcol = st.def === 'over' ? COL.over : '#cfcfcf';
      s += dimension(S.P1, S.P2, 0, 1, 26, A().fmt(W), dcol).svg;
      s += dimension(S.P1, S.P6, -1, 0, 30, A().fmt(H), dcol).svg;
      s += dimension(S.P2, S.P3, 1, 0, 26, A().fmt(tb), dcol).svg;
      if (K.placed.tu) s += dimension(S.P6, S.P5, 0, -1, 26, A().fmt(K.tu), '#cfcfcf').svg;
      for (const x of K.extra) {
        const l = LLINES.find((q) => q[0] === x.line);
        const lbl = x.driven ? '(' + A().fmt(x.value) + ')' : A().fmt(x.value);
        s += dimension(S[l[1]], S[l[2]], 0, -1, 40, lbl, x.driven ? COL.driven : COL.over).svg;
      }
      if (K.modify) {
        const M = K.modify;
        if (M.target === 'tu') { const dm = dimension(S.P6, S.P5, 0, -1, 26, '', COL.sel, { noLabel: true }); s += dm.svg; K.modifyAt = dm.mid; }
        else { const l = LLINES.find((q) => q[0] === M.line); const horiz = l[3] === 'H'; const dm = dimension(S[l[1]], S[l[2]], horiz ? 0 : 1, horiz ? -1 : 0, 40, '', COL.sel, { noLabel: true }); s += dm.svg; K.modifyAt = dm.mid; }
      }
    }
    return s;
  }
  function drawNew() {
    let s = '';
    const st = state();
    if (K.opts.constraints) { /* glyphs per line below */ }
    for (const l of K.lines) {
      const a = K.points[l.a], b = K.points[l.b]; const A1 = w2s(a.u, a.v), B1 = w2s(b.u, b.v);
      s += seg(A1, B1, COL.under, 1.8);
      if (l.rel && K.opts.constraints) { const dx = B1.x - A1.x, dy = B1.y - A1.y, L = Math.hypot(dx, dy) || 1; s += glyph(A1.x + dx * 0.5 - dy / L * 13, A1.y + dy * 0.5 + dx / L * 13, GLYPH[l.rel]); }
    }
    if (K.opts.points) for (const [id, p] of Object.entries(K.points)) { if (p.origin) continue; const q = w2s(p.u, p.v); s += `<rect x="${q.x - 2.5}" y="${q.y - 2.5}" width="5" height="5" fill="${COL.under}"/>`; }
    if (K.tool === 'line' && K.cursor) {
      const c = w2s(K.cursor.u, K.cursor.v);
      if (K.inf && K.inf.align) { const q = K.points[K.inf.align.id]; const Q = w2s(q.u, q.v); s += seg(Q, c, '#4676ff', 1, '2 3'); }
      if (K.chain) {
        const a = K.points[K.chain.from]; const A1 = w2s(a.u, a.v);
        s += seg(A1, c, '#7a95ff', 1.6, '5 3');
        const len = Math.hypot(K.cursor.u - a.u, K.cursor.v - a.v);
        s += `<rect x="${c.x + 14}" y="${c.y + 12}" width="${A().fmt(len, 2).length * 6.6 + 30}" height="18" rx="3" fill="#212121" stroke="#525151"/><text x="${c.x + 20}" y="${c.y + 25}" fill="#cfcfcf" font-size="11" font-family="Inter Silico, Inter">${A().fmt(len, 2)} mm</text>`;
      }
      if (K.inf && K.inf.rel) s += glyph(c.x + 18, c.y - 16, GLYPH[K.inf.rel], '#4676ff');
      if (K.inf && K.inf.snap) s += `<circle cx="${c.x}" cy="${c.y}" r="6" fill="none" stroke="#f2c94c" stroke-width="1.5"/>` + glyph(c.x + 18, c.y - 16, 'cCoincident', '#f2c94c');
      s += `<line x1="${c.x - 8}" y1="${c.y}" x2="${c.x + 8}" y2="${c.y}" stroke="#fff" stroke-width="1"/><line x1="${c.x}" y1="${c.y - 8}" x2="${c.x}" y2="${c.y + 8}" stroke="#fff" stroke-width="1"/>`;
    }
    void st;
    return s;
  }

  // ── Panels while a sketch is open ──
  function defLabel(st) {
    return { full: ['Fully defined', COL.full], under: ['Under defined · ' + st.dof + ' DOF', COL.under], over: ['Over defined', COL.over], empty: ['Empty', '#9f9f9f'] }[st.def];
  }
  function renderCanvasUI() {
    if (!K) return '';
    const st = state(); const [lbl, col] = defLabel(st);
    const name = K.kind === 'bracket' ? 'Bracket profile' : (K.name || 'New sketch');
    const chk = (k, l, keys) => `<button class="check ${K.opts[k] ? 'on' : ''}" data-act="skOpt" data-k="${k}" role="checkbox" aria-checked="${K.opts[k]}"><span class="box"><svg viewBox="0 0 10 10"><path d="M2 5.2l2 2 4-4.4"/></svg></span>${l}${keys ? `<span class="k">${A().kbd(keys)}</span>` : ''}</button>`;
    let html = `<div class="sketch-bar" role="toolbar" aria-label="Sketch"><span class="tag">Sketch</span><span class="strong">${A().esc(name)}</span><span class="subdued">on ${K.plane}</span>
      <span class="state-dot" style="background:${col}"></span><span style="color:${col}">${lbl}</span>
      <button class="btn btn-ghost" data-act="cancelSketch">Cancel</button><button class="btn" style="background:var(--blue);color:#fff" data-act="finishSketch">${icon('check')} Finish Sketch <span class="kbd" style="background:transparent;border-color:rgba(255,255,255,.4);color:#fff">${A().KEYNAME.Mod}↵</span></button></div>`;
    html += `<div class="palette" role="region" aria-label="Sketch palette"><div class="p-title">Sketch palette</div>
      <div class="p-sec"><div class="p-head">Status</div><div style="display:flex;align-items:center;gap:6px;color:${col}"><span class="state-dot" style="background:${col}"></span>${lbl}</div>
        ${st.def === 'under' ? `<div class="subdued" style="font-size:11px;margin-top:3px">Blue curves can still move. ${K.kind === 'bracket' ? 'Dimension the upright’s thickness to fix them.' : 'Add relations or dimensions.'}</div>` : ''}
        ${st.def === 'over' ? `<div class="err-text" style="font-size:11px;margin-top:3px">The red dimension repeats others. Make it driven or delete it.</div>` : ''}</div>
      <div class="p-sec"><div class="p-head">Tool</div>${K.tool ? `<div style="display:flex;align-items:center;gap:6px">${icon({ line: 'line', dimension: 'dimension' }[K.tool] || 'line')}<span class="strong">${K.tool === 'dimension' ? 'Smart Dimension' : 'Line'}</span><span style="flex:1"></span><span class="subdued" style="font-size:11px">Esc leaves</span></div>
        ${K.tool === 'line' ? `<div class="subdued" style="font-size:11px;margin-top:3px">Click, click: a chain. Double-click ends it. Hold ${A().KEYNAME.Mod} to add no relations.</div>${chk('auto', 'Automatic relations')}` : `<div class="subdued" style="font-size:11px;margin-top:3px">Click a line, then type its value.</div>`}` : '<div class="subdued" style="font-size:11px">None. Line <span class="kbd">L</span>, Smart Dimension <span class="kbd">D</span></div>'}
        ${chk('construction', 'Construction', ['X'])}</div>
      <div class="p-sec"><div class="p-head">Show</div>${chk('profile', 'Profile')}${chk('points', 'Points')}${chk('dims', 'Dimensions')}${chk('constraints', 'Constraints')}
        <button class="check" data-act="lookAtSketch">${icon('lookAt')}Look At<span class="k">${A().kbd(['Mod', '8'])}</span></button></div>
      <div class="p-sec"><div class="p-head">Colours</div><div class="legend">
        <i style="background:${COL.under}"></i><span>Under defined</span><i style="background:${COL.full}"></i><span>Fully defined</span>
        <i style="background:${COL.over}"></i><span>Over defined, unsolvable</span><i style="background:${COL.invalid}"></i><span>Invalid (zero length)</span>
        <i style="background:${COL.dangling}"></i><span>Dangling reference</span><i style="background:${COL.driven}"></i><span>Driven dimension</span>
        <i style="background:transparent;border-top:2px dashed #9f9f9f;height:0"></i><span>Construction</span></div></div>
      ${K.kind === 'bracket' && st.def === 'full' && !K.extra.length ? `<div class="p-sec"><button class="btn btn-ghost" style="width:100%;justify-content:flex-start" data-act="skDemoOver">${icon('dimension')} Try: dimension a defined line</button></div>` : ''}
    </div>`;
    if (K.modify && K.modifyAt) {
      const M = K.modify; const x = K.modifyAt.x - 74, y = K.modifyAt.y - 15;
      html += `<div class="modify-box" style="left:${x}px;top:${y}px" role="dialog" aria-label="Modify dimension"><input data-key="sk-mod" data-input="skmod" class="${M.err ? 'bad' : ''}" value="${A().esc(M.text)}" aria-label="Value in millimetres"><span class="unit">mm</span>
        <button class="ab-btn" data-act="skModOk" aria-label="Accept" data-tip="Accept" data-tipk="Enter">${icon('check')}</button><button class="ab-btn" data-act="skModCancel" aria-label="Cancel" data-tip="Cancel" data-tipk="Esc">${icon('close')}</button></div>
        <div class="modify-hint" style="left:${x}px;top:${y + 34}px">${M.err ? `<span class="err-text">${A().esc(M.err)}</span>` : 'A plain number in mm · Enter accepts'}</div>`;
    }
    return html;
  }
  function renderProps() {
    const st = state(); const [lbl, col] = defLabel(st);
    const name = K.kind === 'bracket' ? 'Bracket profile' : (K.name || 'New sketch');
    const rows = (k, v) => `<div class="prop"><span class="k">${k}</span><span class="v">${v}</span></div>`;
    let body = rows('Plane', K.plane) + rows('Definition', `<span style="color:${col}">${lbl}</span>`);
    if (K.kind === 'bracket') body += rows('Curves', '6 lines') + rows('Dimensions', (K.placed.tu ? 4 : 3) + K.extra.length) + rows('Relations', '6 · H, V');
    else body += rows('Curves', K.lines.length + ' line' + (K.lines.length === 1 ? '' : 's')) + rows('Points', Object.keys(K.points).length - 1) + rows('Relations', K.lines.filter((l) => l.rel).length);
    return `<div class="panel-title">Properties</div><div class="panel-body">
      <div class="prop-head"><div class="ph-icon">${icon('sketch')}</div><div><div class="ph-title">${A().esc(name)}</div><div class="ph-sub">Sketch${K.featureId ? ' · <span class="mono">' + K.featureId + '</span>' : ' · new'}</div></div></div>
      <div class="section">Sketch</div>${body}
      ${K.kind === 'bracket' && st.def === 'over' ? `<div class="err-card" role="alert"><div class="ec-head">${icon('error')}<span class="t">Bracket profile is over defined</span><span class="ec-code">sketch_over_defined</span></div><div class="ec-body"><p>The 55 mm length repeats what the 60 mm and the upright's thickness already give.</p><div class="ec-fix">Make it driven, or delete it.</div><div class="ec-actions"><button class="btn btn-secondary" data-act="skMakeDriven">Make it driven</button></div></div></div>` : ''}
      <div class="hint-block">Drag a blue point or curve to move it: what's fully defined stays put. Double-click a dimension to change it.</div>
      <div class="hint-block"><b>Finish Sketch</b> ${A().kbd(['Mod', 'Enter'])} keeps your changes and rebuilds. <b>Cancel</b> drops them.</div></div>`;
  }
  function statusText() {
    const st = state(); const [lbl, col] = defLabel(st);
    const c = K.cursor;
    return `<span class="st-item">Editing ${K.kind === 'bracket' ? 'Bracket profile' : 'sketch'}</span><span class="st-sep"></span><span class="st-item" style="font-variant-numeric:tabular-nums;min-width:150px">${c ? `${K.uL} ${A().fmtF(c.u)} · ${K.vL} ${A().fmtF(c.v)} mm` : '—'}</span><span class="st-sep"></span><span class="st-item" style="color:${col}">${lbl}</span>`;
  }
  function opt(k) { if (K) { K.opts[k] = !K.opts[k]; A().render(); } }
  function demoOver() { K.tool = 'dimension'; K.modify = { target: 'line', line: 'na000003', text: A().fmt(K.dims.W - K.tu, 3), fresh: true }; A().render(); setTimeout(focusModify, 0); }
  function makeDriven() { K.extra.forEach((x) => { x.driven = true; x.conflict = false; }); A().render(); }
  function lookAt() { const fr = FRAMES[K.plane] || {}; VP.animateTo(fr.view || viewFromNormal(frameOf(K))); }

  // New sketch with two lines drawn and a chain under way (prototype state 5n).
  function seedNew() {
    if (!K) return;
    K.points.pt1 = { u: 0, v: 0 }; delete K.points.pt1;
    K.points.a = { u: 0, v: 0 };
    K.points.b = { u: 50, v: 0 }; K.points.c = { u: 50, v: 30 };
    delete K.points.a;
    K.lines = [{ id: 'ln1', a: 'origin', b: 'b', rel: 'H' }, { id: 'ln2', a: 'b', b: 'c', rel: 'V' }];
    K.chain = { from: 'c' }; K.tool = 'line'; K.cursor = { u: 12, v: 30 }; K.inf = { rel: 'H', snap: null, align: null };
    document.getElementById('canvas-wrap').classList.add('tool-line');
    A().render();
  }

  window.SK = { enter, finish, cancel, esc, state, overlay, renderCanvasUI, renderProps, statusText, onMove, onClick, onDbl, setTool, modifyInput, modifyAccept, overdefineChoice, opt, demoOver, makeDriven, lookAt, seedNew, FRAMES, get K() { return K; } };
  window.SK.cancelModify = () => { if (K && K.modify) { K.modify = null; A().render(); } };
})();
