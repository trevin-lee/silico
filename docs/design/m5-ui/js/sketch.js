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
  const PID = { P1: 'pa000001', P2: 'pa000002', P3: 'pa000003', P4: 'pa000004', P5: 'pa000005', P6: 'pa000006' };
  // The profile's fixed dimensions (its width, height, and base thickness), each by ID, and the line it measures.
  const FIXED = { ka000007: { line: 'na000001', a: 'P1', b: 'P2', key: 'W' }, ka000008: { line: 'na000006', a: 'P6', b: 'P1', key: 'H' }, ka000009: { line: 'na000002', a: 'P2', b: 'P3', key: 'tb' } };
  const TU_DIM = 'ka00000a';
  // What defines the upright's thickness: its own dimension, while it drives, and any dimension
  // added on na000003 or na000005 that drives. More than one over-defines the sketch.
  function definers() { return (K.placed.tu && !K.tuDriven ? [TU_DIM] : []).concat(K.extra.filter((x) => !x.driven).map((x) => x.id)); }
  function lstate() {
    const defs = definers();
    const over = defs.length > 1;
    const under = new Set(defs.length ? [] : ['na000003', 'na000004', 'na000005', 'P4', 'P5']);
    let dof = defs.length ? 0 : 1;
    for (const [id, f] of Object.entries(FIXED)) if (K.drivenFixed[id]) { under.add(f.line); under.add(f.a); under.add(f.b); dof++; }
    if (K.unjoined) { under.add(K.unjoined.curve); under.add('NEW'); dof += 2; }
    const def = over ? 'over' : under.size ? 'under' : 'full';
    return { def, dof, under, over };
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
      K.extra = []; K.tuDriven = false; K.drivenFixed = {}; K.unjoined = null;
      K.sel = []; K.hover = null; K.measure = null; K.dimHits = [];
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
      A().edit((dd) => { dd.p.tu = k.tu; dd.sketchState.s0000001 = over ? 'over' : st.def === 'under' ? 'under' : 'full'; if (k.unjoined) dd.scenario = 'unjoined'; else if (dd.scenario === 'unjoined') dd.scenario = null; }, { label: 'Edit Bracket profile' });
      if (!(k.tu !== d.p.tu || over || k.unjoined)) A().toast('Bracket profile unchanged');
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
    if (K.pendingDriving) { const id = K.pendingDriving; K.pendingDriving = null; if (v === 'driving') { makeDriving(id); A().toast('Bracket profile is over defined. Finish keeps it, and Legs then fails.', 'warn', 'err'); } else A().render(); return; }
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
    if (!K.tool && K.kind === 'bracket') { const h = hitAt(e); if (JSON.stringify(h) !== JSON.stringify(K.hover)) { K.hover = h; } }
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
    if (!K.tool && K.kind === 'bracket') { selectAt(e); return; }
    if (!K.tool) A().toast('Pick a tool: Line (L), Smart Dimension (D)…');
  }
  function onDbl(e) {
    if (K && K.chain) { K.chain = null; A().render(); return; }
    if (!K || K.kind !== 'bracket' || K.tool) return;
    const h = hitAt(e); if (!h || h.kind !== 'dim') return;
    if (isDriven(h.id)) { A().toast('This dimension is driven: it shows what the sketch measures. Clear Driven (right-click, or Properties) to make it drive.'); return; }
    if (h.id === TU_DIM) { K.sel = []; K.modify = { target: 'tu', text: A().fmt(K.tu, 3), fresh: true }; A().render(); setTimeout(focusModify, 0); return; }
    A().toast('This prototype edits the 5 mm thickness by double-click; every driving dimension opens its Modify box the same way.');
  }
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
    if (K.measure) { closeMeasure(); return true; }
    if (K.sel && K.sel.length) { K.sel = []; A().render(); return true; }
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
      const x = st.over && K.extra.find((e) => e.line === id && !e.driven);
      if (x || (st.over && ['na000001', 'na000003', 'na000005'].includes(id))) return COL.over;
      return st.under.has(id) ? COL.under : COL.full;
    };
    K.dimHits = [];
    const picked = (kind, id) => K.sel.some((x) => x.kind === kind && x.id === id) || (K.measure && K.measure.picks.some((x) => x.kind === kind && x.id === id));
    const hot = (kind, id) => K.hover && K.hover.kind === kind && K.hover.id === id;
    // An open end (unjoined) leaves no profile: SolidWorks and Fusion shade only closed regions.
    if (K.opts.profile && st.def !== 'over' && !K.unjoined) {
      s += `<polygon points="${['P1', 'P2', 'P3', 'P4', 'P5', 'P6'].map((k) => S[k].x.toFixed(1) + ',' + S[k].y.toFixed(1)).join(' ')}" fill="#186add" fill-opacity=".1"/>`;
    }
    for (const [id, a, b] of LLINES) {
      const hov = K.hoverLine === id || (K.modify && K.modify.line === id) || hot('line', id) || picked('line', id) || (K.menuHl && K.menuHl.id === id);
      if (hov) s += seg(S[a], S[b], COL.sel, 6, null).replace('stroke-linecap', 'stroke-opacity=".35" stroke-linecap');
      s += seg(S[a], S[b], hov ? '#ffffff' : colOf(id), hov ? 2.4 : 1.8);
    }
    if (K.opts.points || K.unjoined) for (const [k, p] of Object.entries(S)) {
      const c = st.under.has(k) ? COL.under : st.def === 'over' ? COL.over : COL.full;
      const on = hot('point', k) || picked('point', k);
      if (on) s += `<rect x="${(p.x - 6).toFixed(1)}" y="${(p.y - 6).toFixed(1)}" width="12" height="12" rx="2" fill="${COL.sel}" fill-opacity=".35"/>`;
      if (K.opts.points) s += `<rect x="${(p.x - 2.5).toFixed(1)}" y="${(p.y - 2.5).toFixed(1)}" width="5" height="5" fill="${on ? '#ffffff' : c}"/>`;
      // An unjoined end: a second point at the same place, drawn open, with check's warning beside it.
      if (K.unjoined && K.unjoined.point === k) {
        s += `<rect x="${(p.x - 5).toFixed(1)}" y="${(p.y - 5).toFixed(1)}" width="10" height="10" fill="none" stroke="${COL.under}" stroke-width="1.4"/>`;
        s += glyph(p.x + 16, p.y - 16, 'warn', '#ff7a0c');
      }
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
      // Each dimension's label is its pick target: a click selects it, a right-click gives its menu.
      const dim = (id, a, b, nx, ny, off, value, col) => {
        const driven = id === TU_DIM ? K.tuDriven : FIXED[id] ? K.drivenFixed[id] : (K.extra.find((x) => x.id === id) || {}).driven;
        const lbl = driven ? '(' + A().fmt(value) + ')' : A().fmt(value);
        const on = hot('dim', id) || picked('dim', id);
        const dm = dimension(a, b, nx, ny, off, lbl, on ? '#ffffff' : driven ? COL.driven : col, { box: on });
        const tw = lbl.length * 6.6 + 10;
        K.dimHits.push({ id, x: dm.mid.x - tw / 2, y: dm.mid.y - 9, w: tw, h: 18 });
        return (on ? `<rect x="${(dm.mid.x - tw / 2 - 3).toFixed(1)}" y="${(dm.mid.y - 12).toFixed(1)}" width="${tw + 6}" height="24" rx="4" fill="${COL.sel}" fill-opacity=".35"/>` : '') + dm.svg;
      };
      s += dim('ka000007', S.P1, S.P2, 0, 1, 26, W, dcol);
      s += dim('ka000008', S.P1, S.P6, -1, 0, 30, H, dcol);
      s += dim('ka000009', S.P2, S.P3, 1, 0, 26, tb, dcol);
      if (K.placed.tu) s += dim(TU_DIM, S.P6, S.P5, 0, -1, 26, K.tu, '#cfcfcf');
      for (const x of K.extra) {
        const l = LLINES.find((q) => q[0] === x.line);
        s += dim(x.id, S[l[1]], S[l[2]], 0, -1, 40, x.value, st.over && !x.driven ? COL.over : '#cfcfcf');
      }
      if (K.modify) {
        const M = K.modify;
        if (M.target === 'tu') { const dm = dimension(S.P6, S.P5, 0, -1, 26, '', COL.sel, { noLabel: true }); s += dm.svg; K.modifyAt = dm.mid; }
        else { const l = LLINES.find((q) => q[0] === M.line); const horiz = l[3] === 'H'; const dm = dimension(S[l[1]], S[l[2]], horiz ? 0 : 1, horiz ? -1 : 0, 40, '', COL.sel, { noLabel: true }); s += dm.svg; K.modifyAt = dm.mid; }
      }
    }
    s += drawMeasure(S);
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
        ${st.def === 'under' ? `<div class="subdued" style="font-size:11px;margin-top:3px">Blue curves can still move. ${K.kind === 'bracket' ? (K.unjoined ? `${K.unjoined.curve}’s end is unjoined: join it to close the profile.` : 'Dimension the upright’s thickness to fix them.') : 'Add relations or dimensions.'}</div>` : ''}
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
    if (K.kind === 'bracket' && K.measure) return measureProps();
    if (K.kind === 'bracket' && K.sel.length) return selectionProps();
    const st = state(); const [lbl, col] = defLabel(st);
    const name = K.kind === 'bracket' ? 'Bracket profile' : (K.name || 'New sketch');
    const rows = (k, v) => `<div class="prop"><span class="k">${k}</span><span class="v">${v}</span></div>`;
    let body = rows('Plane', K.plane) + rows('Definition', `<span style="color:${col}">${lbl}</span>`);
    if (K.kind === 'bracket') body += rows('Curves', '6 lines') + rows('Points', 6 + (K.unjoined ? 1 : 0)) + rows('Dimensions', (K.placed.tu ? 4 : 3) + K.extra.length + (K.extra.some((x) => x.driven) || K.tuDriven || Object.values(K.drivenFixed).some(Boolean) ? ` <span class="subdued">· ${K.extra.filter((x) => x.driven).length + (K.tuDriven ? 1 : 0) + Object.values(K.drivenFixed).filter(Boolean).length} driven</span>` : '')) + rows('Relations', '6 · H, V');
    else body += rows('Curves', K.lines.length + ' line' + (K.lines.length === 1 ? '' : 's')) + rows('Points', Object.keys(K.points).length - 1) + rows('Relations', K.lines.filter((l) => l.rel).length);
    return `<div class="panel-title">Properties</div><div class="panel-body">
      <div class="prop-head"><div class="ph-icon">${icon('sketch')}</div><div><div class="ph-title">${A().esc(name)}</div><div class="ph-sub">Sketch${K.featureId ? ' · <span class="mono">' + K.featureId + '</span>' : ' · new'}</div></div></div>
      <div class="section">Sketch</div>${body}
      ${K.kind === 'bracket' && K.unjoined ? unjoinedCard() : ''}
      ${K.kind === 'bracket' && st.def === 'over' ? `<div class="err-card" role="alert"><div class="ec-head">${icon('error')}<span class="t">Bracket profile is over defined</span><span class="ec-code">sketch_over_defined</span></div><div class="ec-body"><p>The 55 mm length repeats what the 60 mm and the upright's thickness already give.</p><div class="ec-fix">Make it driven, or delete it.</div><div class="ec-actions"><button class="btn btn-secondary" data-act="skMakeDriven">Make it driven</button></div></div></div>` : ''}
      <div class="hint-block">Drag a blue point or curve to move it: what's fully defined stays put. Double-click a dimension to change it.</div>
      <div class="hint-block"><b>Finish Sketch</b> ${A().kbd(['Mod', 'Enter'])} keeps your changes and rebuilds. <b>Cancel</b> drops them.</div></div>`;
  }
  function statusText() {
    const st = state(); const [lbl, col] = defLabel(st);
    const c = K.cursor;
    // SolidWorks' status bar gives the selection's common measure (docs/ux/solidworks.md, §7).
    const one = K.kind === 'bracket' && K.sel && K.sel.length ? selectionSummary() : '';
    return `<span class="st-item">Editing ${K.kind === 'bracket' ? 'Bracket profile' : 'sketch'}</span><span class="st-sep"></span><span class="st-item" style="font-variant-numeric:tabular-nums;min-width:150px">${c ? `${K.uL} ${A().fmtF(c.u)} · ${K.vL} ${A().fmtF(c.v)} mm` : '—'}</span><span class="st-sep"></span>${one ? `<span class="st-item strong">${one}</span><span class="st-sep"></span>` : ''}<span class="st-item" style="color:${col}">${lbl}</span>`;
  }
  function opt(k) { if (K) { K.opts[k] = !K.opts[k]; A().render(); } }
  function demoOver() { K.tool = 'dimension'; K.modify = { target: 'line', line: 'na000003', text: A().fmt(K.dims.W - K.tu, 3), fresh: true }; A().render(); setTimeout(focusModify, 0); }
  function makeDriven() { K.extra.forEach((x) => { x.driven = true; x.conflict = false; }); A().render(); }
  // ───────── Selection, Driven, Unjoin, and Measure in the bracket's sketch ─────────
  const lineOf = (id) => LLINES.find((l) => l[0] === id);
  const linesAt = (pk) => LLINES.filter((l) => l[1] === pk || l[2] === pk).map((l) => l[0]);
  function canvasPt(e) { const r = document.getElementById('gl').getBoundingClientRect(); return { x: e.clientX - r.left, y: e.clientY - r.top }; }
  function hitAt(e) {
    if (K.kind !== 'bracket') return null;
    const m = canvasPt(e); const P = lpoints();
    let best = null;
    for (const k of Object.keys(P)) { const q = w2s(...P[k]); const d = Math.hypot(q.x - m.x, q.y - m.y); if (d < 7 && (!best || d < best.d)) best = { d, k }; }
    if (best) return { kind: 'point', id: best.k };
    const dh = K.dimHits.find((h) => m.x >= h.x - 2 && m.x <= h.x + h.w + 2 && m.y >= h.y - 2 && m.y <= h.y + h.h + 2);
    if (dh) return { kind: 'dim', id: dh.id };
    const l = nearestLine(e);
    return l ? { kind: 'line', id: l } : null;
  }
  const same = (a, b) => a && b && a.kind === b.kind && a.id === b.id;
  function selectAt(e) {
    const h = hitAt(e);
    if (K.measure) {
      if (!h) { K.measure.picks = []; A().render(); return; }
      if (h.kind === 'dim') { A().toast('Measure takes curves and points. A dimension shows its own value.'); return; }
      const i = K.measure.picks.findIndex((x) => same(x, h));
      if (i >= 0) K.measure.picks.splice(i, 1); // a click on a pick takes it out, as SolidWorks' Measure does
      else if (K.measure.picks.length < 2) K.measure.picks.push(h);
      else K.measure.picks[1] = h;
      A().render(); return;
    }
    if (!h) { if (K.sel.length) { K.sel = []; A().render(); } return; }
    if (e.ctrlKey || e.metaKey) { const i = K.sel.findIndex((x) => same(x, h)); if (i >= 0) K.sel.splice(i, 1); else K.sel.push(h); }
    else K.sel = [h];
    A().render();
  }
  function isDriven(id) { return id === TU_DIM ? K.tuDriven : FIXED[id] ? !!K.drivenFixed[id] : !!(K.extra.find((x) => x.id === id) || {}).driven; }
  function dimInfo(id) {
    const { W, H, tb } = K.dims;
    if (FIXED[id]) return { value: { W, H, tb }[FIXED[id].key], line: FIXED[id].line };
    if (id === TU_DIM) return { value: K.tu, line: 'na000005' };
    const x = K.extra.find((q) => q.id === id); return x ? { value: x.value, line: x.line } : null;
  }
  // Each member in the sketch's own coordinates (u, v), as the `sketch` read gives them.
  function geomOf(h) {
    const P = lpoints();
    if (h.kind === 'point') return { kind: 'point', p: P[h.id] };
    const l = lineOf(h.id); return { kind: 'line', a: P[l[1]], b: P[l[2]] };
  }
  const nameOf = (h) => (h.kind === 'point' ? PID[h.id] : h.id);
  // A negative value's minus is a true minus (U+2212), as the app writes a direction's.
  const mm = (v) => A().fmt(Math.abs(v) < 5e-10 ? 0 : v, 3).replace('-', '\u2212') + ' mm';
  const deg = (v) => A().fmt(v, 3) + '°';
  function segPt(p, a, b) { const dx = b[0] - a[0], dy = b[1] - a[1], L = dx * dx + dy * dy; let t = L ? ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / L : 0; t = Math.max(0, Math.min(1, t)); return [a[0] + t * dx, a[1] + t * dy]; }
  const dist = (p, q) => Math.hypot(p[0] - q[0], p[1] - q[1]);
  function one(h) {
    const g = geomOf(h); const r = [];
    if (g.kind === 'line') {
      const du = g.b[0] - g.a[0], dv = g.b[1] - g.a[1];
      r.push(['Length', mm(Math.hypot(du, dv)), 'len']);
      r.push(['Angle', deg(((Math.atan2(dv, du) * 180 / Math.PI) + 360) % 180) + ` <span class="subdued">to ${K.uL}</span>`, 'ang', deg(((Math.atan2(dv, du) * 180 / Math.PI) + 360) % 180)]);
      r.push(['Δ' + K.uL + ', Δ' + K.vL, `${mm(du)}, ${mm(dv)}`, 'd']);
    } else {
      const f = frameOf(K); const w = f.o.clone().addScaledVector(f.U, g.p[0]).addScaledVector(f.V, g.p[1]);
      r.push(['Position', `${K.uL} ${A().fmt(g.p[0], 3)} · ${K.vL} ${A().fmt(g.p[1], 3)} mm`, 'pos']);
      r.push(['In the model', `(${[w.x, w.y, w.z].map((c) => A().fmt(c, 3)).join(', ')}) mm`, 'model']);
    }
    return r;
  }
  // Two members: Fusion's Results (the shortest distance; the angle of two lines), with SolidWorks' dX and dY.
  function pair(a, b) {
    const A1 = geomOf(a), B1 = geomOf(b); const r = []; let draw = null;
    if (A1.kind === 'point' && B1.kind === 'point') {
      r.push(['Distance', mm(dist(A1.p, B1.p)), 'dist']); r.push(['Δ' + K.uL + ', Δ' + K.vL, `${mm(B1.p[0] - A1.p[0])}, ${mm(B1.p[1] - A1.p[1])}`, 'delta']);
      draw = { p: A1.p, q: B1.p, delta: true };
    } else if (A1.kind === 'line' && B1.kind === 'line') {
      const u = [A1.b[0] - A1.a[0], A1.b[1] - A1.a[1]], v = [B1.b[0] - B1.a[0], B1.b[1] - B1.a[1]];
      const cr = u[0] * v[1] - u[1] * v[0], dt = u[0] * v[0] + u[1] * v[1];
      const ang = Math.atan2(Math.abs(cr), Math.abs(dt)) * 180 / Math.PI;
      const cands = [[A1.a, segPt(A1.a, B1.a, B1.b)], [A1.b, segPt(A1.b, B1.a, B1.b)], [segPt(B1.a, A1.a, A1.b), B1.a], [segPt(B1.b, A1.a, A1.b), B1.b]].sort((x, y) => dist(...x) - dist(...y));
      const md = dist(...cands[0]);
      if (Math.abs(cr) < 1e-12 * Math.hypot(...u) * Math.hypot(...v)) {
        const n = Math.abs((B1.a[0] - A1.a[0]) * u[1] - (B1.a[1] - A1.a[1]) * u[0]) / Math.hypot(...u);
        r.push(['Angle', deg(0) + ' <span class="subdued">· parallel</span>', 'ang', deg(0)]); r.push(['Distance', mm(n) + ' <span class="subdued">· between the lines</span>', 'dist', mm(n)]);
        if (Math.abs(md - n) > 1e-9) r.push(['Minimum', mm(md), 'min']);
        draw = { p: cands[0][0], q: segPt(cands[0][0], ...(dist(cands[0][0], A1.a) < 1e-9 || dist(cands[0][0], A1.b) < 1e-9 ? [B1.a.map((c, i) => c - u[i] * 1e3), B1.a.map((c, i) => c + u[i] * 1e3)] : [A1.a, A1.b])) };
      } else {
        r.push(['Angle', deg(ang), 'ang']); r.push(['Minimum', mm(md) + (md < 1e-9 ? ' <span class="subdued">· they meet</span>' : ''), 'min', mm(md)]);
        draw = { p: cands[0][0], q: cands[0][1], angle: { A1, B1, ang } };
      }
    } else {
      const [P1, L1] = A1.kind === 'point' ? [A1, B1] : [B1, A1];
      const q = segPt(P1.p, L1.a, L1.b);
      r.push(['Distance', mm(dist(P1.p, q)) + ' <span class="subdued">· shortest</span>', 'dist', mm(dist(P1.p, q))]);
      r.push(['Δ' + K.uL + ', Δ' + K.vL, `${mm(q[0] - P1.p[0])}, ${mm(q[1] - P1.p[1])}`, 'delta']);
      draw = { p: P1.p, q, delta: true };
    }
    return { rows: r, draw };
  }
  function valueRow(k, v, key, copy) {
    const plain = copy || v.replace(/<[^>]+>[^<]*<\/[^>]+>/g, '').trim();
    return `<div class="prop" data-mhl="${key}"><span class="k">${k}</span><span class="v"><span class="measure-v" data-act="copy" data-text="${A().esc(plain)}" data-tip="Copy" data-tipd="Copies ${A().esc(plain)}">${v}</span></span></div>`;
  }
  const memberIcon = (h) => ({ line: 'line', point: 'point', dim: 'dimension' }[h.kind]);
  const memberSub = (h) => (h.kind === 'line' ? 'Line' : h.kind === 'point' ? 'Point' : 'Dimension') + ' · Bracket profile';
  function selectionSummary() {
    if (K.sel.length === 2 && K.sel.every((x) => x.kind !== 'dim')) { const r = pair(K.sel[0], K.sel[1]).rows; return `${r[0][0]} ${r[0][3] || r[0][1].replace(/<[^>]+>[^<]*<\/[^>]+>/g, '')}`; }
    if (K.sel.length !== 1) return K.sel.length + ' selected';
    const h = K.sel[0];
    if (h.kind === 'dim') { const i = dimInfo(h.id); return `${h.id} · ${isDriven(h.id) ? 'driven ' : ''}${A().fmt(i.value)} mm`; }
    const r = one(h)[0]; return `${nameOf(h)} · ${r[0]} ${r[1]}`;
  }
  function endRows(h) {
    // A point's curves, each with Unjoin: SolidWorks lists a shared end's Merge relation under the point,
    // and deleting it parts them; Fusion puts Delete Coincident on the line's menu (docs/ux, M5.10).
    const pk = h.id; let out = '';
    for (const l of linesAt(pk)) {
      const detached = K.unjoined && K.unjoined.point === pk && K.unjoined.curve === l;
      out += `<div class="prop"><span class="k">${detached ? 'Ends at' : 'Shared by'}</span><span class="v"><span class="mono">${l}</span>${detached ? ` <span class="subdued">· ${K.unjoined.newId}, unjoined</span>` : ''}
        ${detached || (K.unjoined && K.unjoined.point === pk) ? '' : `<button class="btn btn-ghost" style="margin-left:auto;height:20px" data-act="skUnjoin" data-p="${pk}" data-c="${l}" data-tip="Unjoin ${l}" data-tipd="Its end comes away as a new point at the same place (sketch_unjoin). The other curve keeps ${PID[pk]} and its relations">${icon('unjoin')} Unjoin</button>`}</span></div>`;
    }
    if (K.unjoined && K.unjoined.point === pk) out += `<div style="padding:4px var(--pad)"><button class="btn btn-secondary" data-act="skJoin">${icon('join')} Join ${PID[pk]} and ${K.unjoined.newId}</button></div>`;
    return out;
  }
  function selectionProps() {
    const prop = (k, v) => `<div class="prop"><span class="k">${k}</span><span class="v">${v}</span></div>`;
    const title = `<div class="panel-title">Properties<span class="grow"></span><button class="ab-btn" data-act="skClearSel" data-tip="Clear selection" data-tipk="Esc">${icon('close')}</button></div>`;
    if (K.sel.length === 1) {
      const h = K.sel[0]; let body = '';
      if (h.kind === 'dim') {
        const i = dimInfo(h.id); const dr = isDriven(h.id);
        body += `<div class="section">Dimension</div>${prop('Value', dr ? `<span style="color:var(--g550)">(${A().fmt(i.value)} mm)</span> <span class="subdued">· measured</span>` : `${A().fmt(i.value)} mm`)}${prop('Measures', `Length of <span class="mono">${i.line}</span>`)}
          <div class="prop"><span class="k"></span><button class="check ${dr ? 'on' : ''}" data-act="skDriven" data-id="${h.id}" role="checkbox" aria-checked="${dr}" data-tip="Driven" data-tipd="A driven dimension shows what the sketch measures and holds nothing. Clear it to make the dimension drive">
            <span class="box"><svg viewBox="0 0 10 10"><path d="M2 5.2l2 2 4-4.4"/></svg></span>Driven</button></div>
          <div style="display:flex;gap:6px;padding:8px var(--pad)"><button class="btn btn-ghost" data-act="skDelSel">Delete <span class="kbd">${A().KEYNAME.Del}</span></button></div>
          <div class="hint-block">${dr ? 'Driven: grey, in parentheses, and it holds nothing. Clear <b>Driven</b> to make it drive. If the sketch is already defined there, you\'re asked first.' : 'Driving: it holds the sketch. Check <b>Driven</b> to keep it as a measure only. Double-click it to change its value.'}</div>`;
      } else {
        body += `<div class="section">Measures</div>${one(h).map(([k, v, key, c]) => valueRow(k, v, key, c)).join('')}`;
        if (h.kind === 'point') body += `<div class="section">Ends</div>${endRows(h)}`;
        else { const l = lineOf(h.id); body += `<div class="section">Ends</div>${[l[1], l[2]].map((pk) => { const other = linesAt(pk).find((x) => x !== h.id); const open = K.unjoined && K.unjoined.point === pk; return prop(PID[pk], `${open && K.unjoined.curve === h.id ? `<span class="warn-text">${K.unjoined.newId}, unjoined</span>` : `<span class="subdued">with</span>&nbsp;<span class="mono">${other}</span>`}${open ? '' : `<button class="btn btn-ghost" style="margin-left:auto;height:20px" data-act="skUnjoin" data-p="${pk}" data-c="${h.id}" data-tip="Unjoin this end" data-tipd="This line's end at ${PID[pk]} comes away from ${other}">${icon('unjoin')} Unjoin</button>`}`); }).join('')}`; }
        body += `<div class="hint-block">${A().KEYNAME.Mod}-click a second curve or point for the distance and angle between them. <b>Measure</b> (INSPECT) keeps two and draws them.</div>`;
      }
      return `${title}<div class="panel-body"><div class="prop-head"><div class="ph-icon">${icon(memberIcon(h))}</div><div><div class="ph-title mono" style="font-size:12px">${h.kind === 'dim' ? h.id : nameOf(h)}</div><div class="ph-sub">${memberSub(h)}${h.kind === 'dim' ? (isDriven(h.id) ? ' · driven' : ' · driving') : ''}</div></div></div>${body}</div>`;
    }
    const geo = K.sel.filter((x) => x.kind !== 'dim');
    let body = '';
    if (geo.length === 2 && K.sel.length === 2) body += `<div class="section">Measure</div>${pair(geo[0], geo[1]).rows.map(([k, v, key, c]) => valueRow(k, v, key, c)).join('')}`;
    body += `<div class="section">Selection</div>${K.sel.map((h) => prop(h.kind === 'dim' ? 'Dimension' : h.kind === 'line' ? 'Line' : 'Point', `<span class="mono">${h.kind === 'dim' ? h.id : nameOf(h)}</span>`)).join('')}`;
    return `${title}<div class="panel-body"><div class="prop-head"><div class="ph-icon">${icon('sketch')}</div><div><div class="ph-title">${K.sel.length} selected</div><div class="ph-sub">Bracket profile</div></div></div>${body}</div>`;
  }
  // INSPECT › Measure in a sketch: Fusion's Measure dialog (Selection 1, Selection 2, Results), docked as every dialog is.
  function measureProps() {
    const M = K.measure; const [a, b] = M.picks;
    const selSec = (h, n) => `<div class="section">Selection ${n}</div>${h ? `<div class="prop"><span class="k"></span><div class="selbox"><span class="chip">${icon(memberIcon(h))} ${nameOf(h)}</span><span style="flex:1"></span><span class="subdued" style="font-size:11px">${h.kind === 'line' ? 'Line' : 'Point'}</span></div></div>${one(h).map(([k, v, key, c]) => valueRow(k, v, n + key, c)).join('')}` : `<div class="prop"><span class="k"></span><div class="selbox ${(n === 1 && !a) || (n === 2 && a && !b) ? 'active' : ''}"><span class="subdued">${n === 1 ? 'Click a curve or a point' : 'Click a second one'}</span></div></div>`}`;
    const res = a && b ? pair(a, b).rows.map(([k, v, key, c]) => valueRow(k, v, key, c)).join('') : '<div class="hint-block" style="margin-top:4px">Two selections give the distance between them, and two lines their angle.</div>';
    return `<div class="panel-title">${icon('measure')}<span>Measure</span><span class="grow"></span><button class="ab-btn" data-act="skMeasureClose" aria-label="Close" data-tip="Close" data-tipk="Esc">${icon('close')}</button></div>
      <div class="panel-body">${selSec(a, 1)}${selSec(b, 2)}<div class="section">Results</div>${res}
        <div style="display:flex;gap:6px;padding:10px var(--pad)"><button class="btn btn-secondary" data-act="skMeasureClear" ${M.picks.length ? '' : 'disabled'}>${icon('rebuild')} Clear selection</button></div>
        <div class="hint-block">Click a value to copy it. A third pick replaces Selection 2; a click on a pick takes it out, and one on empty space clears both. Measure changes nothing in the sketch.</div></div>
      <div class="dlg-footer"><button class="btn btn-secondary" data-act="skMeasureClose">Close <span class="kbd">Esc</span></button></div>`;
  }
  let hotKey = null;
  function measureHot(k) { if (k !== hotKey) { hotKey = k; VP.request(); } }
  function drawMeasure(S2) {
    const picks = K.measure ? K.measure.picks : K.sel.length === 2 && K.sel.every((x) => x.kind !== 'dim') ? K.sel : null;
    if (!picks || picks.length !== 2) return '';
    const { draw } = pair(picks[0], picks[1]); if (!draw) return '';
    const P = w2s(...draw.p), Q = w2s(...draw.q); let s = '';
    const col = '#4676ff'; const hotDist = hotKey === 'dist' || hotKey === 'min' || hotKey === 'delta';
    if (draw.delta) { const C = w2s(draw.q[0], draw.p[1]); s += seg(P, C, col, hotKey === 'delta' ? 2 : 1, '2 3') + seg(C, Q, col, hotKey === 'delta' ? 2 : 1, '2 3'); }
    if (dist(draw.p, draw.q) > 1e-9) {
      s += seg(P, Q, col, hotDist ? 2.6 : 1.6, '6 3');
      s += `<circle cx="${P.x}" cy="${P.y}" r="3" fill="${col}"/><circle cx="${Q.x}" cy="${Q.y}" r="3" fill="${col}"/>`;
      const lbl = mm(dist(draw.p, draw.q)); const mx = (P.x + Q.x) / 2, my = (P.y + Q.y) / 2; const tw = lbl.length * 6.6 + 10;
      s += `<rect x="${(mx - tw / 2).toFixed(1)}" y="${(my - 9).toFixed(1)}" width="${tw}" height="18" rx="3" fill="#212121" stroke="${col}"/><text x="${mx.toFixed(1)}" y="${(my + 4).toFixed(1)}" fill="#fff" font-size="11" text-anchor="middle" font-family="Inter Silico, Inter">${lbl}</text>`;
    }
    if (draw.angle) {
      // The angle at the lines' crossing, an arc between them.
      const { A1, B1, ang } = draw.angle; const u = [A1.b[0] - A1.a[0], A1.b[1] - A1.a[1]], v = [B1.b[0] - B1.a[0], B1.b[1] - B1.a[1]];
      const den = u[0] * v[1] - u[1] * v[0]; const t = ((B1.a[0] - A1.a[0]) * v[1] - (B1.a[1] - A1.a[1]) * v[0]) / den; const X = [A1.a[0] + t * u[0], A1.a[1] + t * u[1]];
      const far = (g) => (dist(g.a, X) > dist(g.b, X) ? g.a : g.b); const ua = far(A1), ub = far(B1);
      const ra = Math.atan2(ua[1] - X[1], ua[0] - X[0]), rb = Math.atan2(ub[1] - X[1], ub[0] - X[0]); const R = 8;
      const pts = []; let d0 = rb - ra; while (d0 > Math.PI) d0 -= 2 * Math.PI; while (d0 < -Math.PI) d0 += 2 * Math.PI;
      for (let i = 0; i <= 16; i++) { const th = ra + d0 * i / 16; const q = w2s(X[0] + R * Math.cos(th), X[1] + R * Math.sin(th)); pts.push(q.x.toFixed(1) + ',' + q.y.toFixed(1)); }
      s += `<polyline points="${pts.join(' ')}" fill="none" stroke="${col}" stroke-width="${hotKey === 'ang' ? 2.6 : 1.6}"/>`;
      const mth = ra + d0 / 2; const L = w2s(X[0] + (R + 5) * Math.cos(mth), X[1] + (R + 5) * Math.sin(mth)); const lbl = deg(ang); const tw = lbl.length * 6.6 + 10;
      s += `<rect x="${(L.x - tw / 2).toFixed(1)}" y="${(L.y - 9).toFixed(1)}" width="${tw}" height="18" rx="3" fill="#212121" stroke="${col}"/><text x="${L.x.toFixed(1)}" y="${(L.y + 4).toFixed(1)}" fill="#fff" font-size="11" text-anchor="middle" font-family="Inter Silico, Inter">${lbl}</text>`;
    }
    void S2;
    return s;
  }
  function unjoinedCard() {
    const u = K.unjoined;
    return `<div class="err-card warn" role="alert"><div class="ec-head">${icon('warn')}<span class="t">Ends not joined at ${PID[u.point]}</span><span class="ec-code">ends_not_joined</span></div><div class="ec-body">
      <p>Points ${PID[u.point]} and ${u.newId}, which end lines ${linesAt(u.point).filter((l) => l !== u.curve).join(', ')} and ${u.curve}, are at one place but aren't one point, so the profile is open there. Legs won't build from it until they're joined or the gap is closed.</p>
      <div class="ec-kv"><span class="k">Elements</span><span><span class="el-chip">${PID[u.point]}</span> <span class="el-chip">${u.newId}</span></span></div>
      <div class="ec-fix">Join them, or drag ${u.curve}'s end away and close the profile another way.</div>
      <div class="ec-actions"><button class="btn btn-secondary" data-act="skJoin">${icon('join')} Join them</button><button class="btn btn-ghost" data-act="skSelPoint" data-p="${u.point}">Select ${PID[u.point]}</button></div></div></div>`;
  }
  function setDriven(id, driven) {
    if (driven) {
      if (id === TU_DIM) K.tuDriven = true; else if (FIXED[id]) K.drivenFixed[id] = true; else K.extra.find((x) => x.id === id).driven = true;
      A().render(); A().toast(`${id} is driven: it measures and holds nothing.`); return;
    }
    if (FIXED[id]) { K.drivenFixed[id] = false; A().render(); A().toast(`${id} drives again.`); return; }
    // Making it drive where the sketch is already defined asks first, as SolidWorks' Make Dimension Driven? does.
    const others = definers().filter((x) => x !== id);
    if (others.length) { K.pendingDriving = id; A().S.modal = { type: 'overdefine', reverse: true, sketch: 'Bracket profile', value: A().fmt(dimInfo(id).value), dim: id }; A().render(); return; }
    makeDriving(id);
    A().toast(`${id} drives now. ${state().def === 'full' ? 'Bracket profile is fully defined.' : ''}`, 'check');
  }
  function makeDriving(id) {
    if (id === TU_DIM) K.tuDriven = false;
    else { const x = K.extra.find((q) => q.id === id); x.driven = false; if (definers().length === 1) K.tu = x.line === 'na000003' ? K.dims.W - x.value : x.value; }
    A().render();
  }
  function del(list) {
    const L = list || K.sel; if (!L.length) return false;
    const dims = L.filter((x) => x.kind === 'dim');
    if (dims.length !== L.length) { A().toast('Delete removes curves and points too (sketch_remove). This prototype deletes dimensions.'); return true; }
    for (const h of dims) {
      if (FIXED[h.id]) { A().toast('This prototype deletes the 5 mm and 55 mm dimensions; the others delete the same way.'); return true; }
      if (h.id === TU_DIM) { K.placed.tu = false; K.tuDriven = false; } else K.extra = K.extra.filter((x) => x.id !== h.id);
    }
    K.sel = []; A().render();
    A().toast(`Deleted ${dims.map((x) => x.id).join(', ')}. ${state().def === 'under' ? 'Bracket profile is under defined: the upright can move.' : ''}`);
    return true;
  }
  function unjoin(pk, curve) {
    if (K.unjoined) { A().toast('This prototype unjoins one end at a time: join the first, then unjoin another.'); return; }
    K.unjoined = { point: pk, curve, newId: 'pa000008' };
    K.sel = []; A().render();
    A().toast(`Unjoined ${curve} at ${PID[pk]}: its end is ${K.unjoined.newId} now, at the same place.`, 'check');
  }
  function join() { if (!K.unjoined) return; const u = K.unjoined; K.unjoined = null; A().render(); A().toast(`Joined: ${u.newId} merged into ${PID[u.point]} (sketch_merge).`, 'check'); }
  function measure() {
    if (!K || K.kind !== 'bracket') { A().toast('Measure in a sketch: this prototype measures Bracket profile (Edit Sketch on it).'); return; }
    K.tool = null; K.modify = null; K.measure = { picks: K.sel.filter((x) => x.kind !== 'dim').slice(0, 2) }; K.sel = []; A().render();
  }
  function closeMeasure() { if (K) { K.measure = null; hotKey = null; A().render(); } }
  function clearMeasure() { if (K && K.measure) { K.measure.picks = []; A().render(); } }
  function menuHover(hl) { const id = hl ? hl.id : null; if ((K.menuHl && K.menuHl.id) !== id) { K.menuHl = hl || null; VP.request(); } }
  function contextItems(e) {
    const common = [{ label: 'End chain', keys: ['Esc'], disabled: !K.chain, run: () => esc() }, { label: 'Smart Dimension', icon: 'dimension', keys: ['D'], run: () => setTool('dimension') }, { label: 'Measure', icon: 'measure', run: () => measure() },
      'sep', { label: 'Finish Sketch', icon: 'check', keys: ['Mod', 'Enter'], run: () => finish() }, { label: 'Cancel sketch', run: () => cancel() }];
    const h = K.kind === 'bracket' && !K.tool ? hitAt(e) : null;
    if (!h) return common;
    if (!K.measure && !K.sel.some((x) => same(x, h))) { K.sel = [h]; A().render(); }
    let items = [];
    if (h.kind === 'dim') {
      const dr = isDriven(h.id); const i = dimInfo(h.id);
      items = [{ head: `Dimension · ${dr ? '(' + A().fmt(i.value) + ')' : A().fmt(i.value)} mm` },
        { label: (dr ? '✓ ' : '   ') + 'Driven', run: () => setDriven(h.id, !dr) },
        { label: 'Edit value…', icon: 'edit', sub: dr ? 'driven' : 'double-click', disabled: dr, run: () => onDbl({ clientX: e.clientX, clientY: e.clientY }) },
        'sep', { label: 'Delete', danger: true, keys: ['Del'], run: () => del([h]) }];
    } else if (h.kind === 'point') {
      const ls = linesAt(h.id);
      items = [{ head: `Point ${PID[h.id]} · 2 lines end here` }];
      if (K.unjoined && K.unjoined.point === h.id) items.push({ label: `Join ${PID[h.id]} and ${K.unjoined.newId}`, icon: 'join', run: () => join() });
      else ls.forEach((l) => items.push({ label: `Unjoin ${l}`, icon: 'unjoin', sub: `${ls.find((x) => x !== l)} keeps ${PID[h.id]}`, hover: true, hl: { id: l }, run: () => unjoin(h.id, l) }));
    } else {
      const l = lineOf(h.id); const m = canvasPt(e); const ea = w2s(...lpoints()[l[1]]), eb = w2s(...lpoints()[l[2]]);
      const end = Math.hypot(m.x - ea.x, m.y - ea.y) <= Math.hypot(m.x - eb.x, m.y - eb.y) ? l[1] : l[2];
      const other = linesAt(end).find((x) => x !== h.id);
      items = [{ head: `Line ${h.id}` }];
      if (!(K.unjoined && K.unjoined.point === end)) items.push({ label: 'Unjoin this end', icon: 'unjoin', sub: `at ${PID[end]}, from ${other}`, run: () => unjoin(end, h.id) });
      else items.push({ label: `Join ${PID[end]} and ${K.unjoined.newId}`, icon: 'join', run: () => join() });
    }
    return items.concat(['sep'], common);
  }
  // Prototype states 9a to 9f: the three sketch items on Bracket profile.
  function protoState(k, openBracket) {
    const at = (ms, fn) => setTimeout(fn, ms);
    const S0 = A().S;
    const go = (fn) => { openBracket(true); at(300, () => { enter({ featureId: 's0000001', plane: 'XZ', mode: 'edit' }); at(450, fn); }); };
    const clientOf = (p) => { const r = document.getElementById('gl').getBoundingClientRect(); return { clientX: r.left + p.x, clientY: r.top + p.y }; };
    const dimAt = (id) => { const hh = K.dimHits.find((x) => x.id === id); return clientOf({ x: hh.x + hh.w / 2, y: hh.y + hh.h / 2 }); };
    const ptAt = (pk) => clientOf(w2s(...lpoints()[pk]));
    const withDriven = () => { K.extra.push({ id: 'ka00000d', line: 'na000003', value: 55, driven: true }); };
    if (k === '9a') { go(() => { withDriven(); K.sel = [{ kind: 'dim', id: 'ka00000d' }]; A().render(); at(80, () => { const e = dimAt('ka00000d'); window.PROTO.menu(e.clientX + 6, e.clientY + 6, contextItems(e)); }); }); return; }
    if (k === '9b') { go(() => { withDriven(); K.sel = [{ kind: 'dim', id: 'ka00000d' }]; A().render(); setDriven('ka00000d', false); }); return; }
    if (k === '9c') { go(() => { K.sel = [{ kind: 'point', id: 'P4' }]; A().render(); at(80, () => { const e = ptAt('P4'); window.PROTO.menu(e.clientX + 8, e.clientY + 8, contextItems(e)); }); }); return; }
    if (k === '9d') { go(() => { unjoin('P4', 'na000004'); }); return; }
    if (k === '9e') { go(() => { measure(); K.measure.picks = [{ kind: 'point', id: 'P6' }, { kind: 'line', id: 'na000002' }]; A().render(); }); return; }
    if (k === '9f') { go(() => { K.sel = [{ kind: 'line', id: 'na000004' }]; A().render(); }); return; }
    void S0;
  }

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

  window.SK = { enter, finish, cancel, esc, state, overlay, renderCanvasUI, renderProps, statusText, onMove, onClick, onDbl, setTool, modifyInput, modifyAccept, overdefineChoice, opt, demoOver, makeDriven, lookAt, seedNew, FRAMES, get K() { return K; },
    contextItems, setDriven, unjoin, join, del, measure, closeMeasure, clearMeasure, menuHover, measureHot, protoState };
  window.SK.cancelModify = () => { if (K && K.modify) { K.modify = null; A().render(); } };
})();
