/* Silico UI prototype: state, model, and panels.
   Each panel is re-rendered from state on every change, as egui redraws a frame
   from the app's state: no panel keeps state of its own. */
(function () {
  const $ = (s, r) => (r || document).querySelector(s);
  const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const MAC = /Mac|iPhone|iPad/.test(navigator.platform);
  const KEYNAME = { Mod: MAC ? '⌘' : 'Ctrl', Shift: MAC ? '⇧' : 'Shift', Alt: MAC ? '⌥' : 'Alt', Enter: '↵', Del: MAC ? '⌫' : 'Del', Esc: 'Esc' };
  const kbd = (keys) => (keys && keys.length ? `<span class="keys">${keys.map((k) => `<span class="kbd">${esc(KEYNAME[k] || k)}</span>`).join('')}</span>` : '');
  const fmt = (n, d = 2) => Number(n).toLocaleString('en-US', { minimumFractionDigits: 0, maximumFractionDigits: d });
  const fmtF = (n, d = 2) => Number(n).toLocaleString('en-US', { minimumFractionDigits: d, maximumFractionDigits: d });

  // ───────── Commands: one registry for menus, tooltips, the shortcut sheet, and search ─────────
  const CMD = {
    newDoc: { label: 'New part…', keys: ['Mod', 'N'], group: 'File', icon: 'newDoc', desc: 'Asks where to save it, then opens it' },
    open: { label: 'Open…', keys: ['Mod', 'O'], group: 'File', icon: 'open' },
    save: { label: 'Save', keys: ['Mod', 'S'], group: 'File', icon: 'save', desc: 'Writes your edits to the file' },
    saveAs: { label: 'Save As…', keys: ['Mod', 'Shift', 'S'], group: 'File' },
    revert: { label: 'Revert to saved', keys: [], group: 'File', desc: 'Drops every unsaved edit' },
    close: { label: 'Close', keys: ['Mod', 'W'], group: 'File' },
    undo: { label: 'Undo', keys: ['Mod', 'Z'], group: 'Edit', icon: 'undo' },
    redo: { label: 'Redo', keys: MAC ? ['Mod', 'Shift', 'Z'] : ['Mod', 'Y'], group: 'Edit', icon: 'redo' },
    search: { label: 'Search commands', keys: ['S'], group: 'Edit', icon: 'search', desc: 'Every command by name, with its shortcut' },
    rename: { label: 'Rename', keys: ['F2'], group: 'Edit' },
    del: { label: 'Delete', keys: ['Del'], group: 'Edit' },
    rebuild: { label: 'Rebuild', keys: ['Mod', 'B'], group: 'Rebuild', icon: 'rebuild', desc: 'Rebuilds what changed. Resumes a stopped rebuild' },
    rebuildAll: { label: 'Rebuild everything', keys: ['Mod', 'Shift', 'B'], group: 'Rebuild', desc: 'Every feature, ignoring the cache' },
    stop: { label: 'Stop rebuild', keys: ['Esc'], group: 'Rebuild', icon: 'stop' },
    whatsWrong: { label: "What's wrong", keys: [], group: 'Rebuild', icon: 'error', desc: 'Every problem in the document' },
    createSketch: { label: 'Create Sketch', keys: [], group: 'Create', icon: 'sketch', desc: 'Pick an origin plane or a flat face, then sketch on it' },
    pad: { label: 'Pad', keys: [], group: 'Create', icon: 'pad', desc: "Extrudes a sketch's profile into solid" },
    pocket: { label: 'Pocket', keys: [], group: 'Create', icon: 'pocket', desc: "Cuts a sketch's profile out of the solid" },
    revolve: { label: 'Revolve', keys: [], group: 'Create', icon: 'revolve', desc: 'Sweeps a profile around an axis' },
    groove: { label: 'Groove', keys: [], group: 'Create', icon: 'groove', desc: 'Cuts a profile swept around an axis' },
    box: { label: 'Box', keys: [], group: 'Create', icon: 'box' },
    cylinder: { label: 'Cylinder', keys: [], group: 'Create', icon: 'cylinder' },
    sphere: { label: 'Sphere', keys: [], group: 'Create', icon: 'sphere' },
    cone: { label: 'Cone', keys: [], group: 'Create', icon: 'cone' },
    torus: { label: 'Torus', keys: [], group: 'Create', icon: 'torus' },
    fillet: { label: 'Fillet', keys: [], group: 'Modify', icon: 'fillet', desc: 'Rounds the selected edges' },
    chamfer: { label: 'Chamfer', keys: [], group: 'Modify', icon: 'chamfer', desc: 'Bevels the selected edges' },
    union: { label: 'Union', keys: [], group: 'Modify', icon: 'union' },
    subtract: { label: 'Subtract', keys: [], group: 'Modify', icon: 'subtract' },
    intersect: { label: 'Intersect', keys: [], group: 'Modify', icon: 'intersect' },
    measure: { label: 'Measure', keys: [], group: 'Inspect', icon: 'measure', desc: 'Distance, angle, and size of what you select' },
    filterFace: { label: 'Faces', keys: ['X'], group: 'Selection', icon: 'selFace', desc: 'Toggles whether faces can be selected' },
    filterEdge: { label: 'Edges', keys: ['E'], group: 'Selection', icon: 'selEdge', desc: 'Toggles whether edges can be selected' },
    filterVertex: { label: 'Vertices', keys: ['V'], group: 'Selection', icon: 'selVertex', desc: 'Toggles whether vertices can be selected' },
    clearSel: { label: 'Clear selection', keys: ['Esc'], group: 'Selection' },
    selectOther: { label: 'Select Other', keys: ['Right-click'], group: 'Selection', desc: 'What lies under the pointer, nearest first. Tab or the wheel steps' },
    hide: { label: 'Hide or show', keys: ['H'], group: 'Selection', desc: 'The selected body, sketch, or plane' },
    fit: { label: 'Fit', keys: ['F6'], group: 'View', icon: 'fit' },
    home: { label: 'Home view', keys: ['Home'], group: 'View', icon: 'home' },
    vFront: { label: 'Front', keys: ['Mod', '1'], group: 'View' },
    vBack: { label: 'Back', keys: ['Mod', '2'], group: 'View' },
    vLeft: { label: 'Left', keys: ['Mod', '3'], group: 'View' },
    vRight: { label: 'Right', keys: ['Mod', '4'], group: 'View' },
    vTop: { label: 'Top', keys: ['Mod', '5'], group: 'View' },
    vBottom: { label: 'Bottom', keys: ['Mod', '6'], group: 'View' },
    vIso: { label: 'Isometric', keys: ['Mod', '7'], group: 'View' },
    lookAt: { label: 'Look At', keys: ['Mod', '8'], group: 'View', icon: 'lookAt', desc: 'Turns the view square to the selected face or the sketch' },
    orbit: { label: 'Orbit', keys: ['Shift', 'Middle-drag'], group: 'View', icon: 'orbit' },
    pan: { label: 'Pan', keys: ['Middle-drag'], group: 'View', icon: 'pan' },
    zoom: { label: 'Zoom to the pointer', keys: ['Wheel'], group: 'View', icon: 'zoom' },
    sLine: { label: 'Line', keys: ['L'], group: 'Sketch', icon: 'line', desc: 'Click, click: a chain of lines. Double-click ends it' },
    sRect: { label: 'Rectangle', keys: ['R'], group: 'Sketch', icon: 'rect' },
    sCircle: { label: 'Circle', keys: ['C'], group: 'Sketch', icon: 'circle' },
    sArc: { label: 'Arc', keys: ['A'], group: 'Sketch', icon: 'arc' },
    sPoint: { label: 'Point', keys: ['P'], group: 'Sketch', icon: 'point' },
    sTrim: { label: 'Trim', keys: ['T'], group: 'Sketch', icon: 'trim' },
    sExtend: { label: 'Extend', keys: [], group: 'Sketch', icon: 'extend' },
    sSplit: { label: 'Split', keys: [], group: 'Sketch', icon: 'split' },
    sDim: { label: 'Smart Dimension', keys: ['D'], group: 'Sketch', icon: 'dimension', desc: 'Pick a line, a circle, or two entities, then type the value' },
    sConstruction: { label: 'Construction', keys: ['X'], group: 'Sketch', icon: 'construction', desc: 'Toggles the selected curves between profile and construction' },
    sNoRel: { label: 'Sketch without relations', keys: ['Mod', 'hold'], group: 'Sketch', desc: 'Holding it stops the pointer adding relations' },
    finishSketch: { label: 'Finish Sketch', keys: ['Mod', 'Enter'], group: 'Sketch', icon: 'check' },
    ok: { label: 'OK', keys: ['Enter'], group: 'Dialogs' },
    cancel: { label: 'Cancel', keys: ['Esc'], group: 'Dialogs' },
    shortcuts: { label: 'Keyboard shortcuts', keys: ['?'], group: 'Help', icon: 'help' },
  };
  const FEATURE_ICON = { sketch: 'sketch', pad: 'pad', pocket: 'pocket', fillet: 'fillet', chamfer: 'chamfer', revolve: 'revolve', groove: 'groove', box: 'box' };
  const FEATURE_KIND = { sketch: 'Sketch', pad: 'Pad', pocket: 'Pocket', fillet: 'Fillet', chamfer: 'Chamfer' };

  // ───────── Documents ─────────
  function bracketFeatures() {
    return [
      { id: 's0000001', type: 'sketch', name: 'Bracket profile', plane: 'XZ', inputs: [] },
      { id: 'p0000001', type: 'pad', name: 'Legs', inputs: ['s0000001'] },
      { id: 's0000002', type: 'sketch', name: 'Base hole centres', plane: 'XY', inputs: [] },
      { id: 'h0000001', type: 'pocket', name: 'Base holes', inputs: ['p0000001', 's0000002'] },
      { id: 's0000003', type: 'sketch', name: 'Upright hole centres', plane: 'YZ', inputs: [] },
      { id: 'h0000002', type: 'pocket', name: 'Upright holes', inputs: ['h0000001', 's0000003'] },
      { id: 'f0000001', type: 'fillet', name: 'Inside corner', inputs: ['h0000002'] },
    ];
  }
  // Who goes with a feature when it's suppressed (Fable's rule: its true children).
  const CHILDREN = { s0000001: ['p0000001', 'h0000001', 'h0000002', 'f0000001'], p0000001: ['h0000001', 'h0000002', 'f0000001'], s0000002: ['h0000001'], s0000003: ['h0000002'], h0000001: [], h0000002: [], f0000001: [] };
  let docSeq = 1;
  function makeDoc(kind, file, path) {
    const d = {
      uid: 'd' + docSeq++, kind, file, path,
      rev: kind === 'bracket' ? 12 : 1, modelRev: kind === 'bracket' ? 12 : 1,
      p: { W: 60, H: 50, tb: 5, tu: 5, D: 40, r: 5, rev: true },
      features: kind === 'bracket' ? bracketFeatures() : [],
      suppressed: {}, scenario: null, sketchState: { s0000003: 'under' }, newSketches: {},
      outside: {}, rollback: null,
      undo: [], redo: [], disk: 'unchanged', pendingDisk: null,
      health: {}, problems: [], built: null,
    };
    d.saved = snap(d);
    evaluate(d); d.built = d.next; d.health = d.next.health; d.problems = d.next.problems;
    return d;
  }
  function snap(d) { return JSON.stringify({ p: d.p, s: d.suppressed, sc: d.scenario, f: d.features.map((f) => [f.id, f.name]), ss: d.sketchState, ns: d.newSketches }); }
  function restore(d, s) { const o = JSON.parse(s); d.p = o.p; d.suppressed = o.s; d.scenario = o.sc; d.sketchState = o.ss; d.newSketches = o.ns || {}; const names = Object.fromEntries(o.f); d.features = o.f.map(([id]) => d.features.find((f) => f.id === id) || newSketchFeature(d, id)).filter(Boolean); d.features.forEach((f) => { f.name = names[f.id]; }); }
  function newSketchFeature(d, id) { const ns = d.newSketches[id]; return ns ? { id, type: 'sketch', name: ns.name, plane: ns.plane, inputs: [] } : null; }
  const dirty = (d) => snap(d) !== d.saved;

  // ───────── The model: health, problems, and what to build (stands in for the core) ─────────
  function evaluate(d) {
    const health = {}, problems = [];
    const roll = d.rollback == null ? d.features.length : d.rollback;
    const supp = new Set();
    for (const id of Object.keys(d.suppressed)) if (d.suppressed[id]) { supp.add(id); (CHILDREN[id] || []).forEach((c) => supp.add(c)); }
    d.features.forEach((f, i) => {
      if (i >= roll) health[f.id] = 'rolled';
      else if (supp.has(f.id)) health[f.id] = 'suppressed';
      else health[f.id] = 'healthy';
    });
    const p = d.p, ok = (id) => health[id] === 'healthy';
    if (d.kind === 'bracket') {
      if (ok('s0000001') && d.sketchState.s0000001 === 'over') {
        health.s0000001 = 'error';
        problems.push({ feature: 's0000001', severity: 'error', code: 'sketch_over_defined', title: 'Bracket profile is over defined',
          message: 'Its dimensions conflict: the 55 mm length repeats what the 60 mm and 5 mm dimensions already give.',
          elements: ['ka00000d (55 mm)', 'ka000007 (60 mm)'], fix: 'Make the 55 mm dimension driven, or delete it.' });
        ['p0000001', 'h0000001', 'h0000002', 'f0000001'].forEach((id) => { if (ok(id)) health[id] = 'blocked'; });
      }
      if (ok('s0000003') && d.sketchState.s0000003 === 'under') { /* builds: SolidWorks builds from under-defined sketches */ }
      const holesMiss = p.D < 12 - 2.75;
      if (ok('h0000001') && holesMiss) {
        health.h0000001 = 'error';
        problems.push({ feature: 'h0000001', severity: 'error', code: 'sweep_no_change', title: 'Base holes cut nothing',
          message: `At a ${fmt(p.D)} mm pad, both holes of Base hole centres lie outside the solid, so the pocket would change nothing.`,
          elements: ['cb000001', 'cb000002'], param: `Legs · length = ${fmt(p.D)} mm`, fix: 'Make Legs at least 9.25 mm long, or move the holes.' });
      }
      if (ok('h0000002') && holesMiss) {
        health.h0000002 = 'error';
        problems.push({ feature: 'h0000002', severity: 'error', code: 'sweep_no_change', title: 'Upright holes cut nothing',
          message: `At a ${fmt(p.D)} mm pad, both holes of Upright hole centres lie outside the solid.`,
          elements: ['cc000001', 'cc000002'], param: `Legs · length = ${fmt(p.D)} mm`, fix: 'Make Legs at least 9.25 mm long, or move the holes.' });
      }
      if (ok('f0000001') && d.scenario === 'brokenRef') {
        health.f0000001 = 'error';
        problems.push({ feature: 'f0000001', severity: 'error', code: 'reference_broken', title: 'Inside corner lost its edge',
          message: 'Its edge p0000001.pad.Edge(G s0000001.na000003, G s0000001.na000004) no longer exists: Bracket profile\'s line na000004 was split in two at revision ' + d.rev + '. Silico never re-points a reference on its own.',
          elements: ['p0000001.pad.Edge(G s0000001.na000003, G s0000001.na000004)'], param: 'edges[0]',
          fix: 'Pick the edge it should round now, or remove it from the fillet.',
          candidates: [
            { edge: 'corner', name: 'p0000001.pad.Edge(G s0000001.na000003, G s0000001.na000007)', why: 'Made from the same line na000003 and the lower piece of na000004 (na000007)' },
            { edge: 'top', name: 'p0000001.pad.Edge(G s0000001.na000004, G s0000001.na000005)', why: 'Made from the upper piece of na000004 (kept its ID)' },
          ] });
      }
      if (ok('f0000001') && p.r >= 27.25) {
        health.f0000001 = 'error';
        problems.push({ feature: 'f0000001', severity: 'error', code: 'kernel_failed', title: 'Inside corner failed',
          message: `The kernel couldn't fillet the inside corner with a ${fmt(p.r)} mm radius: the rounded face would run into Upright holes.`,
          elements: ['Edge (inside corner)'], param: `radius = ${fmt(p.r)} mm`, fix: 'Use a smaller radius. It last built at 5 mm, revision 12.' });
      }
      // Downstream of a failed feature: blocked (built from nothing), as M2.4 reports.
      const order = d.features.map((f) => f.id);
      for (const pr of problems) {
        if (pr.feature === 'h0000001' && ok('h0000002')) { /* h2 builds on h1's base */ }
      }
      void order;
    }
    const built = health.p0000001 === 'healthy' && d.kind === 'bracket';
    const geoParams = built ? {
      W: p.W, H: p.H, tb: p.tb, tu: p.tu, D: p.D,
      r: health.f0000001 === 'healthy' ? p.r : 0,
      baseHoles: health.h0000001 === 'healthy', uprightHoles: health.h0000002 === 'healthy',
    } : null;
    d.next = { health, problems, geoParams, rev: d.rev };
    return d.next;
  }

  // ───────── App state ─────────
  const S = {
    view: 'start', noDocVariant: 'start', docs: [], active: null,
    sel: [], hover: null, filter: { face: true, edge: true, vertex: true },
    expanded: { root: true, origin: false, bodies: true, sketches: true },
    vis: { body: true, planes: { XY: false, XZ: false, YZ: false }, sketches: {} },
    selFeature: null, hoverFeature: null, selRow: null,
    dialog: null, sketch: null, pickPlane: null,
    rebuild: { state: 'done', done: 7, steps: 7, ms: 84, current: null },
    banner: null, menu: null, modal: null, palette: null, tip: null,
    slow: false, toasts: [], workers: 'starting', candHot: null, navMode: null,
    proto: { collapsed: true, current: '1' },
  };
  const doc = () => S.docs.find((d) => d.uid === S.active) || null;
  window.S = S;

  // ───────── Rebuild (stands in for status.rebuild: done of steps, stop, resume) ─────────
  let rbTimer = null;
  function startRebuild(d, opts = {}) {
    clearTimeout(rbTimer);
    evaluate(d);
    const steps = d.features.filter((f) => !['rolled', 'suppressed'].includes(d.next.health[f.id])).length;
    S.rebuild = { state: 'running', done: opts.from || 0, steps, t0: performance.now(), preview: !!opts.preview, rev: d.rev };
    tickRebuild(d);
    render();
  }
  function tickRebuild(d) {
    const per = S.slow ? 650 : (S.rebuild.preview ? 45 : 70);
    rbTimer = setTimeout(() => {
      if (S.rebuild.state !== 'running') return;
      S.rebuild.done++;
      if (S.rebuild.done >= S.rebuild.steps) finishRebuild(d);
      else { renderTimeline(); renderStatus(); renderRight(); tickRebuild(d); }
    }, per);
  }
  function finishRebuild(d) {
    S.rebuild.state = 'done'; S.rebuild.ms = Math.round(performance.now() - S.rebuild.t0) * (S.slow ? 1 : 1);
    d.built = d.next; d.health = d.next.health; d.problems = d.next.problems; d.modelRev = d.rev;
    publish(d);
    render();
  }
  function stopRebuild() {
    if (S.rebuild.state !== 'running') return false;
    clearTimeout(rbTimer); S.rebuild.state = 'stopped';
    toast('Rebuild stopped at step ' + S.rebuild.done + ' of ' + S.rebuild.steps + '. Rebuild (' + KEYNAME.Mod + 'B) resumes it.');
    render(); return true;
  }
  function resumeRebuild() {
    const d = doc(); if (!d) return;
    if (S.rebuild.state === 'stopped') { S.rebuild.state = 'running'; S.rebuild.t0 = performance.now(); tickRebuild(d); render(); }
    else startRebuild(d);
  }
  function publish(d) {
    if (!d || doc() !== d) return;
    const gp = d.built && d.built.geoParams;
    if (S.sketch && S.sketch.rollModel) { VP.setModel(null); return; }
    VP.setModel(gp ? Geo.buildBracket(gp) : null);
    VP.setBodyVisible(S.vis.body);
    syncHighlight();
    syncSketchLines();
  }

  // ───────── Edits (each one undoable; the dialog's edits are one step) ─────────
  function edit(fn, opts = {}) {
    const d = doc(); if (!d) return;
    const before = snap(d);
    fn(d);
    if (snap(d) === before) return;
    if (!opts.noUndo) { d.undo.push({ s: before, rev: d.rev, label: opts.label || 'Edit' }); d.redo = []; }
    d.rev++;
    if (d.disk === 'unchanged' && opts.local !== false) { /* a local edit opens the transaction */ }
    startRebuild(d, { preview: opts.preview });
  }
  function undo() {
    const d = doc(); if (!d || !d.undo.length) { toast('Nothing to undo'); return; }
    if (S.dialog) { cancelDialog(); return; }
    const u = d.undo.pop(); d.redo.push({ s: snap(d), rev: d.rev, label: u.label }); restore(d, u.s); d.rev++;
    toast('Undid: ' + u.label); startRebuild(d);
  }
  function redo() {
    const d = doc(); if (!d || !d.redo.length) { toast('Nothing to redo'); return; }
    const u = d.redo.pop(); d.undo.push({ s: snap(d), rev: d.rev, label: u.label }); restore(d, u.s); d.rev++;
    toast('Redid: ' + u.label); startRebuild(d);
  }
  function save() {
    const d = doc(); if (!d) return;
    if (d.disk === 'changed') { S.modal = { type: 'saveConflict' }; render(); return; }
    if (d.disk === 'gone') { S.modal = { type: 'saveAs' }; render(); return; }
    if (!dirty(d)) { toast('Nothing to save: ' + d.file + ' matches the file.'); return; }
    d.saved = snap(d); d.undo = []; d.redo = [];
    toast('Saved ' + d.file + ' (revision ' + d.rev + ')', 'check');
    render();
  }

  // ───────── Selection ─────────
  function setSel(list) { S.sel = list; S.selFeature = null; S.selRow = null; syncHighlight(); render(); }
  function toggleSel(item) {
    const i = S.sel.findIndex((s) => s.kind === item.kind && s.id === item.id);
    if (i >= 0) S.sel.splice(i, 1); else S.sel.push(item);
    S.selFeature = null; S.selRow = null; syncHighlight(); render();
  }
  function syncHighlight() {
    const d = doc();
    const cand = [];
    let candHot = null;
    const m = VP.model();
    const pr = d && d.problems.find((p) => p.code === 'reference_broken');
    const showCands = pr && (S.selFeature === 'f0000001' || (S.dialog && S.dialog.type === 'fillet') || (S.modal && S.modal.type === 'whatswrong'));
    if (m && showCands) {
      for (const c of pr.candidates) {
        const e = m.edges.find((e) => e.lengthwise && (c.edge === 'corner' ? (e.corner[0] === d.p.tu && e.corner[1] === d.p.tb) : (e.corner[0] === d.p.tu && e.corner[1] === d.p.H)));
        if (e) { cand.push(e.id); if (S.candHot === c.edge) candHot = e.id; }
      }
    }
    const maker = S.hoverFeature || S.selFeature;
    VP.setHighlight({ hover: S.hover, sel: S.sel, cand, candHot, maker });
  }

  // ───────── Toasts and tooltips ─────────
  function toast(text, ic, cls) {
    const id = Math.random();
    S.toasts.push({ id, text, ic: ic || 'info', cls });
    renderToasts();
    setTimeout(() => { S.toasts = S.toasts.filter((t) => t.id !== id); renderToasts(); }, 3800);
  }
  function renderToasts() {
    $('#toasts').innerHTML = S.toasts.map((t) => `<div class="toast ${t.cls || ''}">${icon(t.ic)}<span>${esc(t.text)}</span></div>`).join('');
  }

  // ───────── Rendering ─────────
  function render() {
    const fk = document.activeElement && document.activeElement.dataset && document.activeElement.dataset.key;
    const selStart = fk && document.activeElement.selectionStart, selEnd = fk && document.activeElement.selectionEnd;
    const app = $('#app');
    const d = doc();
    const startView = S.view === 'start' && !(S.noDocVariant === 'canvas' && !d);
    app.classList.toggle('start-view', startView);
    $('#start').hidden = !startView;
    renderAppbar();
    if (startView) { renderStart(); }
    else { renderToolbar(); renderBrowser(); renderRight(); renderTimeline(); renderCanvasUI(); }
    renderStatus(); renderFloating(); renderProto();
    VP.showCube = true;
    VP.request();
    if (fk) {
      const el = document.querySelector(`[data-key="${fk}"]`);
      if (el && el !== document.activeElement) { el.focus(); try { el.setSelectionRange(selStart, selEnd); } catch (e) { /* not a text input */ } }
    }
  }
  window.render = render;

  function renderAppbar() {
    const d = doc();
    const home = S.view === 'start' || !d;
    const tabs = S.docs.map((x) => {
      const err = x.problems.some((p) => p.severity === 'error');
      const out = Object.keys(x.outside).length;
      return `<div class="doc-tab ${x.uid === S.active && S.view === 'doc' ? 'active' : ''}" data-act="tab" data-id="${x.uid}" role="tab" tabindex="0" aria-selected="${x.uid === S.active}" data-tip="${esc(x.path)}" data-tipd="${err ? 'Has errors. ' : ''}${dirty(x) ? 'Unsaved edits. ' : ''}${out ? 'Changed on disk since you opened it.' : ''}">
        ${err ? '<span class="mark" aria-label="has errors"></span>' : out ? '<span class="mark purple" aria-label="changed outside"></span>' : ''}
        <span class="name">${esc(x.file)}${dirty(x) ? '<span class="dirty"> *</span>' : ''}</span>
        <button class="x" data-act="closeTab" data-id="${x.uid}" aria-label="Close ${esc(x.file)}">${icon('close')}</button></div>`;
    }).join('');
    const canUndo = d && d.undo.length && S.view === 'doc', canRedo = d && d.redo.length && S.view === 'doc';
    $('#appbar').innerHTML = `
      <div class="traffic" aria-hidden="true"><span></span><span></span><span></span></div>
      <button class="ab-btn ${home ? 'on' : ''}" data-act="home" data-tip="Home" data-tipd="Recent parts, New, Open, and samples">${icon('home')}</button>
      <button class="ab-btn" data-act="fileMenu" aria-haspopup="menu" data-tip="File">${icon('menu')}<span>File</span></button>
      <button class="ab-btn" data-act="save" ${d && S.view === 'doc' ? '' : 'disabled'} data-tip="Save" data-tipk="Mod S" data-tipd="${d && dirty(d) ? 'Writes your edits to ' + esc(d.file) : 'Nothing unsaved'}">${icon('save')}</button>
      <button class="ab-btn" data-act="undo" ${canUndo ? '' : 'disabled'} data-tip="Undo${canUndo ? ': ' + esc(d.undo[d.undo.length - 1].label) : ''}" data-tipk="Mod Z">${icon('undo')}</button>
      <button class="ab-btn" data-act="redo" ${canRedo ? '' : 'disabled'} data-tip="Redo${canRedo ? ': ' + esc(d.redo[d.redo.length - 1].label) : ''}" data-tipk="${CMD.redo.keys.join(' ')}">${icon('redo')}</button>
      <span class="ab-sep"></span>
      <div id="tabs" role="tablist">${tabs}</div>
      <button class="ab-btn" data-act="newDoc" data-tip="New part" data-tipk="Mod N" style="margin-left:4px">${icon('plus')}</button>
      <span class="ab-spacer"></span>
      <div class="ab-right">
        <button class="ab-btn" data-act="search" data-tip="Search commands" data-tipk="S">${icon('search')}</button>
        <button class="ab-btn" data-act="shortcuts" data-tip="Keyboard shortcuts" data-tipk="?">${icon('help')}</button>
      </div>`;
  }

  function tbBtn(id, opts = {}) {
    const c = CMD[id] || { label: id };
    return `<button class="tb-btn ${opts.on ? 'on' : ''} ${opts.toggle ? 'toggle-on' : ''}" data-act="${opts.act || 'cmd'}" data-cmd="${id}" ${opts.disabled ? 'disabled' : ''}
      aria-label="${esc(c.label)}" aria-pressed="${opts.toggle ? 'true' : opts.pressable ? 'false' : ''}" data-tip="${esc(c.label)}" data-tipk="${(c.keys || []).join(' ')}" data-tipd="${esc(opts.why || c.desc || '')}">${icon(c.icon || 'box', 'lg')}</button>`;
  }
  function renderToolbar() {
    const d = doc(); const inSketch = !!S.sketch;
    $('#toolbar').classList.toggle('disabled', !d);
    const tabs = `<div class="tb-tabs" role="tablist">
      <span class="tb-ws">Design</span>
      <button class="tb-tab ${!inSketch ? 'active' : ''}" role="tab" data-act="noop">Solid</button>
      ${inSketch ? '<button class="tb-tab ctx active" role="tab">Sketch</button>' : ''}
    </div>`;
    let panels;
    if (inSketch) {
      const t = S.sketch.tool;
      panels = `
        <div class="tb-panel"><div class="tb-row">${tbBtn('sLine', { act: 'tool', on: t === 'line' })}${tbBtn('sRect', { act: 'tool', on: t === 'rect' })}${tbBtn('sCircle', { act: 'tool', on: t === 'circle' })}${tbBtn('sArc', { act: 'tool', on: t === 'arc' })}${tbBtn('sPoint', { act: 'tool', on: t === 'point' })}${tbBtn('sConstruction', { act: 'cmd' })}</div><button class="tb-label" data-act="noop">Create ${icon('down')}</button></div>
        <div class="tb-panel"><div class="tb-row">${tbBtn('sTrim', { act: 'tool', on: t === 'trim' })}${tbBtn('sExtend', { act: 'tool' })}${tbBtn('sSplit', { act: 'tool' })}</div><button class="tb-label" data-act="noop">Modify ${icon('down')}</button></div>
        <div class="tb-panel"><div class="tb-row">${tbBtn('sDim', { act: 'tool', on: t === 'dimension' })}
          ${['cHorizontal', 'cVertical', 'cCoincident', 'cTangent', 'cEqual', 'cParallel', 'cPerpendicular', 'cMidpoint', 'cSymmetric', 'cFix'].map((c) => `<button class="tb-btn" data-act="constraint" data-c="${c}" data-tip="${c.slice(1)}" data-tipd="Select the curves or points first">${icon(c, 'lg')}</button>`).join('')}
        </div><button class="tb-label" data-act="noop">Constraints ${icon('down')}</button></div>
        <div class="tb-panel"><div class="tb-row">${tbBtn('measure')}</div><button class="tb-label" data-act="noop">Inspect ${icon('down')}</button></div>
        <div class="tb-finish"><button class="btn-finish" data-act="finishSketch" data-tip="Finish Sketch" data-tipk="Mod Enter" data-tipd="Keeps your changes and rebuilds">${icon('check')}<span>Finish Sketch</span></button></div>`;
    } else {
      const hasEdge = S.sel.some((s) => s.kind === 'edge');
      panels = `
        <div class="tb-panel"><div class="tb-row">${tbBtn('createSketch', { on: !!S.pickPlane })}${tbBtn('pad')}${tbBtn('pocket')}${tbBtn('revolve')}${tbBtn('groove')}</div><button class="tb-label" data-act="createMenu" aria-haspopup="menu">Create ${icon('down')}</button></div>
        <div class="tb-panel"><div class="tb-row">${tbBtn('fillet', { why: hasEdge ? 'Rounds the ' + S.sel.filter((s) => s.kind === 'edge').length + ' selected edge(s)' : 'Rounds edges. Select them first, or after' })}${tbBtn('chamfer')}${tbBtn('union')}${tbBtn('subtract')}${tbBtn('intersect')}</div><button class="tb-label" data-act="modifyMenu" aria-haspopup="menu">Modify ${icon('down')}</button></div>
        <div class="tb-panel"><div class="tb-row">${tbBtn('measure')}</div><button class="tb-label" data-act="noop">Inspect ${icon('down')}</button></div>
        <div class="tb-panel"><div class="tb-row">
          ${tbBtn('filterFace', { act: 'filter', toggle: S.filter.face, pressable: true })}${tbBtn('filterEdge', { act: 'filter', toggle: S.filter.edge, pressable: true })}${tbBtn('filterVertex', { act: 'filter', toggle: S.filter.vertex, pressable: true })}
        </div><button class="tb-label" data-act="selectMenu" aria-haspopup="menu">Select ${icon('down')}</button></div>`;
    }
    $('#toolbar').innerHTML = tabs + `<div class="tb-panels">${panels}</div>`;
  }

  // Browser: Origin, Bodies, Sketches, grouped by type (Fusion). History is the timeline.
  function row(o) {
    const pad = 6 + o.depth * 18;
    return `<div class="row ${o.selected ? 'selected' : ''} ${o.hidden ? 'hidden-obj' : ''} ${o.err ? 'err' : ''}" style="padding-left:${pad}px" data-act="${o.act || 'row'}" data-id="${o.id}" ${o.feature ? `data-feature="${o.feature}"` : ''} tabindex="-1" role="treeitem" aria-expanded="${o.caret == null ? '' : o.caret}" aria-selected="${!!o.selected}">
      ${o.caret == null ? '<span class="caret"></span>' : `<button class="caret ${o.caret ? 'open' : ''}" data-act="expand" data-id="${o.id}" tabindex="-1" aria-label="${o.caret ? 'Collapse' : 'Expand'}">${icon('chevron')}</button>`}
      ${o.eye == null ? '<span class="eye"></span>' : `<button class="eye ${o.eye ? '' : 'off'}" data-act="eye" data-id="${o.id}" tabindex="-1" aria-label="${o.eye ? 'Hide' : 'Show'} ${esc(o.name)}" data-tip="${o.eye ? 'Hide' : 'Show'}" data-tipk="H">${icon(o.eye ? 'eye' : 'eyeOff')}</button>`}
      <span class="r-icon">${icon(o.icon)}</span>
      ${o.prefix ? `<span class="prefix ${o.prefixCls || ''}" data-tip="${esc(o.prefixTip || '')}">${o.prefix}</span>` : ''}
      <span class="r-name">${esc(o.name)}</span>
      ${o.meta ? `<span class="r-meta">${o.meta}</span>` : ''}
      ${o.mark === 'error' ? `<span class="r-err" data-tip="${esc(o.markTip || 'Has an error')}">${icon('error')}</span>` : o.mark === 'warn' ? `<span class="r-warn">${icon('warn')}</span>` : o.mark === 'outside' ? '<span class="state-dot" style="background:var(--purple)" data-tip="Changed on disk since you opened it"></span>' : ''}
    </div>`;
  }
  function renderBrowser() {
    const d = doc();
    const collapsed = $('#main').classList.contains('browser-collapsed');
    if (collapsed) { $('#browser').innerHTML = `<button class="ab-btn" style="margin:2px" data-act="toggleBrowser" aria-label="Show browser" data-tip="Show browser">${icon('chevron')}</button>`; return; }
    let body = '';
    if (!d) {
      body = `<div class="hint-block" style="margin-top:4px">No document open.<br><br><b>New</b> ${kbd(['Mod', 'N'])} or <b>Open</b> ${kbd(['Mod', 'O'])}, or pick a recent part on the Home page.</div>`;
    } else {
      const err = d.problems.some((p) => p.severity === 'error');
      const E = S.expanded;
      body += row({ id: 'root', depth: 0, caret: E.root, icon: 'doc', name: d.file.replace('.silico', ''), meta: dirty(d) ? 'unsaved' : '', mark: err ? 'error' : null, markTip: "Click to see what's wrong", act: 'rootRow', selected: S.selRow === 'root' });
      if (E.root) {
        body += row({ id: 'origin', depth: 1, caret: E.origin, icon: 'folder', name: 'Origin', eye: Object.values(S.vis.planes).some(Boolean) });
        if (E.origin) {
          body += row({ id: 'o-pt', depth: 2, icon: 'origin', name: 'Origin' });
          for (const a of ['X', 'Y', 'Z']) body += row({ id: 'o-' + a, depth: 2, icon: 'axis', name: a + ' axis' });
          for (const pl of ['XY', 'XZ', 'YZ']) body += row({ id: 'pl-' + pl, depth: 2, icon: 'plane', name: pl + ' plane', eye: S.vis.planes[pl], selected: S.selRow === 'pl-' + pl, act: 'planeRow' });
        }
        const bodies = d.built && d.built.geoParams ? 1 : 0;
        body += row({ id: 'bodies', depth: 1, caret: E.bodies, icon: 'folder', name: 'Bodies', meta: bodies ? '' : '0' });
        if (E.bodies) {
          if (bodies) {
            const bodyErr = d.problems.find((p) => p.severity === 'error');
            const lastHealthy = [...d.features].reverse().find((f) => f.type !== 'sketch' && d.health[f.id] === 'healthy');
            body += row({ id: 'body1', depth: 2, icon: 'body', name: 'Body1', meta: lastHealthy ? esc(lastHealthy.name) : '', eye: S.vis.body, hidden: !S.vis.body, selected: S.selRow === 'body1', mark: bodyErr ? 'error' : null, markTip: bodyErr ? bodyErr.title : '', act: 'bodyRow' });
          } else body += `<div class="tree-empty">${d.features.length ? 'No body: ' + (d.problems[0] ? esc(d.problems[0].title) : 'nothing builds one yet') : 'None yet. A pad or a primitive makes one.'}</div>`;
        }
        const sketches = d.features.filter((f) => f.type === 'sketch');
        body += row({ id: 'sketches', depth: 1, caret: E.sketches, icon: 'folder', name: 'Sketches', meta: sketches.length ? '' : '0' });
        if (E.sketches) {
          if (!sketches.length) body += '<div class="tree-empty">None yet. Create Sketch starts one.</div>';
          for (const f of sketches) {
            const st = sketchDefState(d, f.id);
            const prefix = st === 'under' ? '(–)' : st === 'over' ? '(+)' : st === 'unsolvable' ? '(?)' : '';
            const h = d.health[f.id];
            body += row({ id: 'sk-' + f.id, feature: f.id, depth: 2, icon: 'sketch', name: f.name, prefix, prefixCls: st, prefixTip: st === 'under' ? 'Under defined' : st === 'over' ? 'Over defined' : '',
              eye: !!S.vis.sketches[f.id], hidden: !S.vis.sketches[f.id], selected: S.selFeature === f.id, err: h === 'error', mark: h === 'error' ? 'error' : d.outside[f.id] ? 'outside' : null, act: 'featureRow', meta: f.plane });
          }
        }
      }
    }
    $('#browser').innerHTML = `<div class="panel-title"><span>Browser</span><span class="grow"></span><button class="ab-btn" data-act="toggleBrowser" aria-label="Collapse browser" data-tip="Collapse">${icon('chevron')}</button></div><div class="panel-body" role="tree" aria-label="Browser tree">${body}</div>`;
  }
  function sketchDefState(d, id) {
    if (S.sketch && S.sketch.featureId === id && window.SK) return SK.state().def;
    return d.sketchState[id] || 'full';
  }

  // ───────── Right panel: properties, or the docked feature dialog ─────────
  function prop(k, v, opts = {}) {
    return `<div class="prop"><span class="k">${esc(k)}</span><span class="v">${v}${opts.copy ? `<button class="copy" data-act="copy" data-text="${esc(opts.copy)}" aria-label="Copy" data-tip="Copy, as an agent would write it">${icon('copy')}</button>` : ''}</span></div>`;
  }
  function section(t) { return `<div class="section">${esc(t)}</div>`; }
  function errCard(pr, opts = {}) {
    const d = doc();
    const f = d.features.find((x) => x.id === pr.feature);
    let html = `<div class="err-card ${pr.severity === 'warning' ? 'warn' : ''}" role="alert">
      <div class="ec-head">${icon(pr.severity === 'warning' ? 'warn' : 'error')}<span class="t">${esc(pr.title)}</span><span class="ec-code" data-tip="The error's code, as agents receive it">${esc(pr.code)}</span></div>
      <div class="ec-body"><p>${esc(pr.message)}</p>
      <div class="ec-kv"><span class="k">Feature</span><span>${esc(f ? f.name : pr.feature)} <span class="mono" style="opacity:.7">${pr.feature}</span></span>
      ${pr.elements ? `<span class="k">Elements</span><span>${pr.elements.map((e) => `<span class="el-chip">${esc(e.length > 34 ? e.slice(0, 32) + '…' : e)}</span>`).join(' ')}</span>` : ''}
      ${pr.param ? `<span class="k">Parameter</span><span>${esc(pr.param)}</span>` : ''}</div>
      <div class="ec-fix">${esc(pr.fix)}</div>`;
    if (pr.candidates) {
      html += `<div style="font-size:10.5px;color:#ff99a6;margin-bottom:4px;text-transform:uppercase;letter-spacing:.5px">Candidates, closest first</div>`;
      pr.candidates.forEach((c, i) => {
        html += `<div class="cand" data-act="noop" data-cand="${c.edge}"><div class="c-main"><span class="mono">${esc(c.name)}</span><span class="why">${i === 0 ? 'Closest · ' : ''}${esc(c.why)}</span></div><button class="btn btn-secondary" data-act="useCand" data-cand="${c.edge}">Use</button></div>`;
      });
    }
    html += `<div class="ec-actions">
      ${opts.noEdit ? '' : `<button class="btn btn-secondary" data-act="editFeature" data-id="${pr.feature}">${icon('edit')} Edit ${esc(f ? f.name : '')}</button>`}
      ${pr.candidates ? `<button class="btn btn-ghost" data-act="removeRef">Remove the edge</button>` : ''}
      ${opts.noShow ? '' : `<button class="btn btn-ghost" data-act="showFeature" data-id="${pr.feature}">Show in timeline</button>`}
      <button class="btn btn-ghost" data-act="copy" data-text="${esc(JSON.stringify({ code: pr.code, feature: pr.feature, elements: pr.elements, parameter: pr.param, cause: pr.message, suggestion: pr.fix }))}" data-tip="Copies the structured error an agent receives">Copy as JSON</button>
      </div></div></div>`;
    return html;
  }
  function renderRight() {
    const d = doc(); let html;
    if (S.dialog) html = renderDialog(d);
    else if (!d) html = `<div class="panel-title">Properties</div><div class="panel-body"><div class="hint-block">Properties of what you select appear here. Feature dialogs dock here too.</div></div>`;
    else if (S.sketch && window.SK) html = SK.renderProps();
    else if (S.sel.length) html = selectionProps(d);
    else if (S.selFeature) html = featureProps(d, S.selFeature);
    else if (S.selRow === 'body1') html = bodyProps(d);
    else html = docProps(d);
    $('#right').innerHTML = html;
  }
  function docProps(d) {
    const g = VP.model();
    const errs = d.problems.filter((p) => p.severity === 'error');
    return `<div class="panel-title">Properties</div><div class="panel-body">
      <div class="prop-head"><div class="ph-icon">${icon('doc')}</div><div><div class="ph-title">${esc(d.file)}</div><div class="ph-sub mono">${esc(d.path)}</div></div></div>
      ${errs.length ? errs.map((e) => errCard(e)).join('') : ''}
      ${section('Document')}
      ${prop('Revision', d.rev + (d.modelRev !== d.rev ? ` <span class="subdued">· model ${d.modelRev}</span>` : ''))}
      ${prop('Units', 'mm · 1 nm resolution')}
      ${prop('Features', d.features.length)}
      ${prop('Saved', dirty(d) ? `<span class="strong">${d.undo.length} unsaved edit${d.undo.length === 1 ? '' : 's'}</span>` : 'Yes')}
      ${prop('File on disk', d.disk === 'unchanged' ? 'Unchanged' : d.disk === 'changed' ? '<span class="warn-text">Changed by another program</span>' : '<span class="err-text">Gone</span>')}
      ${g ? section('Model') + prop('Bodies', 1) + prop('Volume', fmtF(g.volume, 2) + ' mm³') + prop('Area', fmtF(g.area, 2) + ' mm²') + prop('Size', `${fmt(g.bbox[0])} × ${fmt(g.bbox[1])} × ${fmt(g.bbox[2])} mm`) + prop('Faces', g.faces.length) : section('Model') + `<div class="hint-block">${d.features.length ? 'Nothing is built yet.' : 'An empty part. <b>Create Sketch</b>, pick a plane, and draw a profile; <b>Pad</b> then makes it solid.'}</div>`}
      <div class="hint-block">Click a face, edge, or vertex to inspect it. Double-click a feature in the timeline to edit it. Press <span class="kbd">?</span> for every shortcut.</div>
    </div>`;
  }
  function bodyProps(d) {
    const g = VP.model();
    return `<div class="panel-title">Properties</div><div class="panel-body">
      <div class="prop-head"><div class="ph-icon">${icon('body')}</div><div><div class="ph-title">Body1</div><div class="ph-sub">Solid body</div></div></div>
      ${section('Measures')}${g ? prop('Volume', fmtF(g.volume) + ' mm³') + prop('Area', fmtF(g.area) + ' mm²') + prop('Size', `${fmt(g.bbox[0])} × ${fmt(g.bbox[1])} × ${fmt(g.bbox[2])} mm`) + prop('Faces', g.faces.length) + prop('Edges', g.edges.length) : ''}
      ${section('Made by')}${d.features.filter((f) => f.type !== 'sketch').map((f) => prop(FEATURE_KIND[f.type], `<span class="link" data-act="selectFeature" data-id="${f.id}">${esc(f.name)}</span>`)).join('')}
    </div>`;
  }
  function featureProps(d, id) {
    const f = d.features.find((x) => x.id === id); if (!f) return docProps(d);
    const h = d.health[id]; const p = d.p;
    const pr = d.problems.filter((x) => x.feature === id);
    const healthTxt = { healthy: '<span class="ok-text">Healthy</span>', error: '<span class="err-text">Error</span>', suppressed: 'Suppressed', rolled: 'Rolled back', blocked: '<span class="warn-text">Not built: an earlier feature failed</span>' }[h] || h;
    let rows = '';
    if (f.type === 'sketch') {
      const st = sketchDefState(d, id);
      rows += prop('Plane', f.plane) + prop('Definition', st === 'under' ? '<span style="color:var(--sk-under)">Under defined</span>' : st === 'over' ? '<span class="err-text">Over defined</span>' : 'Fully defined');
      if (id === 's0000001') rows += prop('Curves', '6 lines') + prop('Dimensions', `${fmt(p.W)}, ${fmt(p.H)}, ${fmt(p.tb)}, ${fmt(p.tu)} mm`);
      else rows += prop('Curves', '2 circles, ⌀ 5.5 mm');
    }
    if (f.type === 'pad') rows += prop('Profile', '<span class="link" data-act="selectFeature" data-id="s0000001">Bracket profile</span> · 1 region') + prop('Extent', `Distance · ${fmt(p.D)} mm`) + prop('Direction', p.rev ? 'Reversed (+Y)' : 'Normal (−Y)');
    if (f.type === 'pocket') rows += prop('Profile', `<span class="link" data-act="selectFeature" data-id="${f.inputs[1]}">${esc(d.features.find((x) => x.id === f.inputs[1]).name)}</span> · 2 regions`) + prop('Extent', 'Through all') + prop('Base', esc(d.features.find((x) => x.id === f.inputs[0]).name));
    if (f.type === 'fillet') rows += prop('Edges', '1 · the inside corner') + prop('Radius', fmt(p.r) + ' mm') + prop('Base', 'Upright holes');
    return `<div class="panel-title">Properties</div><div class="panel-body">
      <div class="prop-head"><div class="ph-icon">${icon(FEATURE_ICON[f.type])}</div><div><div class="ph-title">${esc(f.name)}</div><div class="ph-sub">${FEATURE_KIND[f.type]} · <span class="mono">${f.id}</span></div></div></div>
      ${pr.map((x) => errCard(x, { noShow: true })).join('')}
      ${d.outside[id] ? `<div class="err-card warn" style="background:rgba(183,128,255,.12);border-color:#463375"><div class="ec-head" style="color:#e4d6ff">${icon('diff')}<span class="t">Changed on disk</span></div><div class="ec-body" style="color:#e4d6ff"><p>${esc(d.outside[id])}</p><div class="ec-actions"><button class="btn btn-ghost" data-act="ackOutside" data-id="${id}">Mark as seen</button></div></div></div>` : ''}
      ${section('Parameters')}${rows}
      ${section('Health')}${prop('State', healthTxt)}${prop('ID', `<span class="mono">${f.id}</span>`, { copy: f.id })}
      <div style="display:flex;gap:6px;padding:10px var(--pad)">
        <button class="btn btn-secondary" data-act="editFeature" data-id="${id}">${icon('edit')} ${f.type === 'sketch' ? 'Edit Sketch' : 'Edit feature'}</button>
        <button class="btn btn-ghost" data-act="suppress" data-id="${id}">${d.suppressed[id] ? 'Unsuppress' : 'Suppress'}</button>
      </div>
      <div class="hint-block">Hovering a feature in the timeline lights up the faces it made.</div>
    </div>`;
  }
  function featName(d, id) { const f = d.features.find((x) => x.id === id); return f ? f.name : id; }
  function selectionProps(d) {
    const g = VP.model(); if (!g) return docProps(d);
    const items = S.sel.map((s) => ({ s, ref: s.kind === 'face' ? g.faces.find((f) => f.id === s.id) : s.kind === 'edge' ? g.edges.find((e) => e.id === s.id) : g.verts.find((v) => v.id === s.id) })).filter((x) => x.ref);
    if (!items.length) return docProps(d);
    if (items.length === 1) {
      const { s, ref } = items[0]; let rows = '', title, sub, ic;
      if (s.kind === 'face') {
        ic = 'selFace'; title = s.id; sub = ref.kind === 'plane' ? 'Planar face' : 'Cylindrical face';
        rows += prop('Area', fmtF(ref.area) + ' mm²');
        if (ref.kind === 'plane') rows += prop('Normal', `(${ref.normal.join(', ')})`); else rows += prop('Radius', fmtF(ref.radius) + ' mm') + prop('Axis', `(${ref.axis.join(', ')})`);
      } else if (s.kind === 'edge') {
        ic = 'selEdge'; title = s.id; sub = { line: 'Line', circle: 'Circle', arc: 'Arc' }[ref.kind] + ' edge';
        rows += prop('Length', fmtF(ref.length) + ' mm'); if (ref.radius) rows += prop('Radius', fmtF(ref.radius) + ' mm');
      } else { ic = 'selVertex'; title = s.id; sub = 'Vertex'; rows += prop('Position', `(${fmt(ref.p.x)}, ${fmt(ref.p.y)}, ${fmt(ref.p.z)}) mm`); }
      const stable = ref.stable || `p0000001.pad.Vertex(${s.id})`;
      return `<div class="panel-title">Properties<span class="grow"></span><button class="ab-btn" data-act="clearSel" data-tip="Clear selection" data-tipk="Esc">${icon('close')}</button></div><div class="panel-body">
        <div class="prop-head"><div class="ph-icon">${icon(ic)}</div><div><div class="ph-title">${title}</div><div class="ph-sub">${sub} · Body1</div></div></div>
        ${section('Measures')}${rows}
        ${section('Name')}${prop('Stable name', `<span class="mono" title="${esc(stable)}">${esc(stable)}</span>`, { copy: stable })}
        ${prop('Made by', `<span class="link" data-act="selectFeature" data-id="${ref.maker}">${esc(featName(d, ref.maker))}</span>`)}
        ${prop('Stable', '<span class="ok-text">Yes</span> · a feature may store it')}
        <div style="display:flex;gap:6px;padding:10px var(--pad);flex-wrap:wrap">
          ${s.kind === 'face' && ref.kind === 'plane' ? `<button class="btn btn-secondary" data-act="sketchOnFace" data-id="${s.id}">${icon('sketch')} Sketch on this face</button>` : ''}
          ${s.kind === 'edge' ? `<button class="btn btn-secondary" data-act="cmd" data-cmd="fillet">${icon('fillet')} Fillet</button>` : ''}
          <button class="btn btn-ghost" data-act="editFeature" data-id="${ref.maker}">${icon('edit')} Edit ${esc(featName(d, ref.maker))}</button>
        </div>
        <div class="hint-block">${KEYNAME.Mod}-click adds or removes. Right-click › <b>Select Other</b> reaches what's behind.</div></div>`;
    }
    let extra = '';
    if (items.length === 2 && items.every((x) => x.s.kind === 'face' && x.ref.kind === 'plane')) {
      const [a, b] = items.map((x) => x.ref);
      const na = new THREE.Vector3(...a.normal), nb = new THREE.Vector3(...b.normal);
      const ang = Math.acos(Math.min(1, Math.abs(na.dot(nb)))) * 180 / Math.PI;
      if (ang < 1e-6) {
        const pa = a.frame.o, pb = b.frame.o; const dist = Math.abs(pb.clone().sub(pa).dot(na));
        extra = section('Measure') + prop('Distance', fmtF(dist) + ' mm') + prop('Angle', '0° (parallel)');
      } else extra = section('Measure') + prop('Angle', fmtF(ang) + '°');
    }
    return `<div class="panel-title">Properties<span class="grow"></span><button class="ab-btn" data-act="clearSel" data-tip="Clear selection" data-tipk="Esc">${icon('close')}</button></div><div class="panel-body">
      <div class="prop-head"><div class="ph-icon">${icon('selFace')}</div><div><div class="ph-title">${items.length} selected</div><div class="ph-sub">${['face', 'edge', 'vertex'].map((k) => { const n = items.filter((x) => x.s.kind === k).length; return n ? n + ' ' + k + (n > 1 ? (k === 'vertex' ? 'ices' : 's') : '') : ''; }).filter(Boolean).join(', ')}</div></div></div>
      ${extra}
      ${section('Selection')}${items.map((x) => prop(x.s.id, `<span class="mono subdued">${esc(x.ref.stable || '')}</span>`)).join('')}
      <div style="padding:10px var(--pad)"><button class="btn btn-ghost" data-act="copy" data-text="${esc(JSON.stringify(items.map((x) => x.ref.stable)))}">${icon('copy')} Copy names</button></div></div>`;
  }

  // Docked feature dialog (Fusion's look, SolidWorks' PropertyManager behavior).
  function renderDialog(d) {
    const D = S.dialog; const f = D.featureId ? d.features.find((x) => x.id === D.featureId) : null;
    const title = { pad: 'Pad', pocket: 'Pocket', fillet: 'Fillet' }[D.type];
    const running = S.rebuild.state === 'running';
    const probs = d.next ? d.next.problems : d.problems;
    const relevant = running ? [] : probs;
    let inputs = '';
    if (D.type === 'pad') {
      inputs += `<div class="prop"><span class="k">Profile</span><div class="selbox ${D.activeBox === 'profile' ? 'active' : ''}" data-act="activateBox" data-box="profile" data-tip="Selection box" data-tipd="Click a sketch in the browser or a closed region in the canvas">${D.profile ? `<span class="chip">${icon('sketch')} ${esc(D.profile)} · 1 region</span><span class="grow" style="flex:1"></span><button class="ab-btn" style="height:18px;min-width:18px" data-act="clearProfile" aria-label="Clear">${icon('close')}</button>` : '<span class="subdued">Select a sketch profile</span>'}</div></div>`;
      inputs += `<div class="prop"><span class="k">Extent</span><button class="field select" data-act="noop">Distance ${icon('down')}</button></div>`;
      inputs += `<div class="prop"><span class="k">Distance</span><div class="field ${D.bad ? 'bad' : ''}"><input data-key="dlg-dist" data-input="dist" value="${esc(D.text)}" aria-label="Distance in millimetres" inputmode="decimal" ${D.profile ? '' : 'disabled'}><span class="unit">mm</span></div></div>`;
      if (D.bad) inputs += `<div class="prop"><span></span><span class="err-text" style="font-size:11px;white-space:normal">${esc(D.bad)}</span></div>`;
      inputs += `<div class="prop"><span class="k">Direction</span><div class="seg" role="radiogroup"><button class="${!D.rev ? 'on' : ''}" data-act="dlgRev" data-v="0" role="radio" aria-checked="${!D.rev}" data-tip="Normal (−Y)">${icon('pad')}</button><button class="${D.rev ? 'on' : ''}" data-act="dlgRev" data-v="1" role="radio" aria-checked="${!!D.rev}" data-tip="Reversed (+Y)" style="transform:scaleY(-1)">${icon('pad')}</button></div></div>`;
    } else if (D.type === 'fillet') {
      const pr = d.problems.find((x) => x.code === 'reference_broken');
      inputs += `<div class="prop"><span class="k">Edges</span><div class="selbox ${D.activeBox === 'edges' ? 'active' : ''}" data-act="activateBox" data-box="edges">${pr ? `<span class="chip" style="background:#5c001a;color:#ffd1d6">${icon('error')} 1 missing</span>` : `<span class="chip">${icon('selEdge')} Inside corner edge</span>`}<span style="flex:1"></span><span class="subdued" style="font-size:11px">1</span></div></div>`;
      inputs += `<div class="prop"><span class="k">Radius</span><div class="field ${D.bad ? 'bad' : ''}"><input data-key="dlg-rad" data-input="rad" value="${esc(D.text)}" aria-label="Radius in millimetres" inputmode="decimal"><span class="unit">mm</span></div></div>`;
      if (D.bad) inputs += `<div class="prop"><span></span><span class="err-text" style="font-size:11px;white-space:normal">${esc(D.bad)}</span></div>`;
    } else if (D.type === 'pocket') {
      inputs += `<div class="prop"><span class="k">Profile</span><div class="selbox"><span class="chip">${icon('sketch')} ${esc(featName(d, f.inputs[1]))} · 2 regions</span></div></div>`;
      inputs += `<div class="prop"><span class="k">Extent</span><button class="field select" data-act="noop">Through all ${icon('down')}</button></div>`;
    }
    const st = running
      ? `<div class="dlg-status running"><span class="spinner"></span>Rebuilding the preview · ${S.rebuild.done} of ${S.rebuild.steps}<span style="flex:1"></span><span class="subdued">Esc stops it</span></div>`
      : S.rebuild.state === 'stopped' ? `<div class="dlg-status">${icon('stop')} Preview stopped. Rebuild ${kbd(['Mod', 'B'])} resumes it.</div>`
        : relevant.length ? `<div class="dlg-status" style="color:var(--red)">${icon('error')} The preview has ${relevant.length} error${relevant.length > 1 ? 's' : ''}. OK keeps the feature with them.</div>`
          : `<div class="dlg-status">${icon('check')} Preview up to date · revision ${d.rev} · ${S.rebuild.ms} ms</div>`;
    const okDisabled = !!D.bad || (D.type === 'pad' && !D.profile);
    return `<div class="panel-title">${icon(D.type)}<span>${title}</span><span class="grow"></span><button class="ab-btn" data-act="cancelDialog" aria-label="Cancel" data-tip="Cancel" data-tipk="Esc">${icon('close')}</button></div>
      <div class="panel-body">
        <div class="prop"><span class="k">Name</span><div class="field"><input data-key="dlg-name" data-input="name" value="${esc(D.name)}" aria-label="Feature name"></div></div>
        ${section('Inputs')}${inputs}
        ${section('Preview')}${st}
        ${relevant.map((x) => errCard(x, { noEdit: true, noShow: true })).join('')}
        ${D.type === 'pad' && D.profile ? '<div class="hint-block">Drag the arrow in the canvas to change the distance. The view stays free to move while the dialog is open.</div>' : ''}
        ${D.type === 'pad' && !D.profile ? '<div class="hint-block">The profile box is active (blue). Click a sketch in the browser to fill it.</div>' : ''}
      </div>
      <div class="dlg-footer"><button class="btn btn-secondary" data-act="cancelDialog">Cancel <span class="kbd">Esc</span></button><button class="btn btn-primary" data-act="okDialog" ${okDisabled ? 'disabled' : ''}>OK <span class="kbd">↵</span></button></div>`;
  }
  function openDialog(type, featureId) {
    const d = doc(); if (!d) return;
    if (S.sketch) { toast('Finish or cancel the sketch first'); return; }
    if (S.dialog) cancelDialog();
    const f = featureId && d.features.find((x) => x.id === featureId);
    S.dialog = { type, featureId, name: f ? f.name : type === 'pad' ? 'Pad1' : 'Fillet1', openSnap: snap(d), openUndo: d.undo.length, openRev: d.rev, activeBox: null };
    if (type === 'pad') { S.dialog.profile = featureId ? 'Bracket profile' : null; S.dialog.text = featureId ? String(d.p.D) : '10'; S.dialog.rev = d.p.rev; if (!featureId) S.dialog.activeBox = 'profile'; }
    if (type === 'fillet') { S.dialog.text = String(d.p.r); }
    S.sel = []; S.selFeature = featureId || null;
    syncHighlight(); render();
    const inp = document.querySelector('[data-key="dlg-dist"],[data-key="dlg-rad"]'); if (inp && !inp.disabled) { inp.focus(); inp.select(); }
  }
  function parseValue(t, what) {
    const s = t.trim();
    if (!/^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(s)) return { err: `"${s}" isn't a number. Type a plain number in mm, like 40 or 12.5.` };
    const v = Number(s);
    if (!(v > 0)) return { err: `${what} must be more than 0 mm.` };
    if (v > 1e6) return { err: `${what} must be at most 1,000,000 mm.` };
    return { v };
  }
  function dialogInput(key, value) {
    const D = S.dialog; if (!D) return;
    if (key === 'name') { D.name = value; renderRightKeepFocus(); return; }
    D.text = value;
    const r = parseValue(value, key === 'dist' ? 'The distance' : 'The radius');
    D.bad = r.err || null;
    if (!r.err) {
      if (key === 'dist') edit((d) => { d.p.D = r.v; }, { noUndo: true, preview: true });
      if (key === 'rad') edit((d) => { d.p.r = r.v; }, { noUndo: true, preview: true });
    } else render();
  }
  function renderRightKeepFocus() { render(); }
  function okDialog() {
    const D = S.dialog; const d = doc(); if (!D || !d) return;
    if (D.bad) return;
    if (D.featureId) { const f = d.features.find((x) => x.id === D.featureId); if (f && D.name.trim()) f.name = D.name.trim(); }
    if (D.type === 'pad' && !D.featureId) { toast('Prototype: a new pad from ' + (D.profile || 'nothing') + ' would be added here.'); S.dialog = null; render(); return; }
    if (snap(d) !== D.openSnap) { d.undo.push({ s: D.openSnap, rev: D.openRev, label: 'Edit ' + D.name }); d.redo = []; }
    S.dialog = null; syncHighlight(); render();
    toast('Applied ' + D.name + '. ' + KEYNAME.Mod + 'Z undoes it as one step.', 'check');
  }
  function cancelDialog() {
    const D = S.dialog; const d = doc(); if (!D || !d) return;
    S.dialog = null;
    if (snap(d) !== D.openSnap) { restore(d, D.openSnap); d.rev++; startRebuild(d); }
    syncHighlight(); render();
  }

  // ───────── Timeline ─────────
  function renderTimeline() {
    const d = doc();
    if (!d) { $('#timeline').innerHTML = `<span class="tl-label">Timeline</span><span class="tl-empty">Features appear here, in order, as you add them.</span>`; return; }
    const running = S.rebuild.state !== 'done';
    let built = 0;
    const roll = d.rollback == null ? d.features.length : d.rollback;
    let items = '';
    d.features.forEach((f, i) => {
      const nextH = d.next ? d.next.health[f.id] : d.health[f.id];
      let cls = [], badge = '';
      const counted = !['rolled', 'suppressed'].includes(nextH);
      if (running && counted) {
        if (built < S.rebuild.done) cls.push(stateCls(nextH));
        else if (built === S.rebuild.done && S.rebuild.state === 'running') cls.push('building');
        else cls.push(S.rebuild.state === 'stopped' ? 'stopped' : 'queued');
        built++;
      } else cls.push(stateCls(nextH));
      if (f.type === 'sketch') cls.push('sketch');
      if (S.selFeature === f.id) cls.push('selected');
      const h = running ? null : d.health[f.id];
      if (h === 'error') badge = '<span class="badge err" aria-hidden="true"></span>';
      else if (h === 'blocked') badge = '<span class="badge warn" aria-hidden="true"></span>';
      else if (d.outside[f.id]) badge = '<span class="badge outside" aria-hidden="true"></span>';
      if (i === roll) items += rollBar();
      const pr = d.problems.find((p) => p.feature === f.id);
      const healthWord = { healthy: 'Healthy', error: 'Error', suppressed: 'Suppressed', rolled: 'Rolled back', blocked: 'Not built: an earlier feature failed' }[h || nextH];
      items += `<button class="tl-item ${cls.join(' ')}" data-act="tlItem" data-id="${f.id}" aria-label="${esc(f.name)}, ${FEATURE_KIND[f.type]}, ${healthWord}"
        data-tip="${esc(f.name)}" data-tipd="${FEATURE_KIND[f.type]} · ${healthWord}${pr ? ' — ' + esc(pr.title) : ''}${d.outside[f.id] ? ' — changed on disk: ' + esc(d.outside[f.id]) : ''}. Double-click to edit.">${icon(FEATURE_ICON[f.type], 'lg')}${badge}</button>`;
    });
    if (roll >= d.features.length) items += rollBar();
    if (!d.features.length) items = '<span class="tl-empty">Features appear here, in order, as you add them.</span>';
    const errs = d.problems.filter((p) => p.severity === 'error').length;
    $('#timeline').innerHTML = `<span class="tl-label">Timeline</span><div class="tl-track" role="toolbar" aria-label="Features in order">${items}</div>
      <div class="tl-right">${errs ? `<button class="st-item err-text" data-act="whatsWrong">${icon('error')} ${errs} error${errs > 1 ? 's' : ''}</button>` : ''}<span>${d.features.length} features</span><button class="ab-btn" data-act="noop" data-tip="Timeline options">${icon('gear')}</button></div>`;
  }
  function rollBar() { return `<div class="tl-roll" data-act="noop" data-roll="1" role="separator" aria-label="Rollback bar" data-tip="Rollback bar" data-tipd="Drag to see the part as it was at an earlier feature. (Onshape's; Fusion's marker is out of M5)"></div>`; }
  function stateCls(h) { return { error: 'err', blocked: 'warn', suppressed: 'suppressed', rolled: 'rolled', healthy: '' }[h] || ''; }

  // ───────── Status bar ─────────
  function renderStatus() {
    const d = doc(); const R = S.rebuild; let left = '';
    if (!d) left = `<span class="st-item">${S.workers === 'ready' ? `<span class="state-dot" style="background:var(--green)"></span>Kernel and sketch solver ready` : '<span class="spinner"></span>Starting the kernel and sketch solver…'}</span>`;
    else if (R.state === 'running') left = `<span class="st-item strong"><span class="spinner"></span>${R.preview ? 'Rebuilding the preview' : 'Rebuilding'} · ${R.done} of ${R.steps}</span><span class="progress" aria-hidden="true"><i style="width:${(R.done / Math.max(1, R.steps)) * 100}%"></i></span>`;
    else if (R.state === 'stopped') left = `<span class="st-item warn-text">${icon('stop')} Rebuild stopped at ${R.done} of ${R.steps}</span>`;
    else {
      const errs = d.problems.filter((p) => p.severity === 'error').length;
      left = errs ? `<button class="st-item err-text" data-act="whatsWrong" data-tip="What's wrong" data-tipd="Lists every problem, with what to do">${icon('error')} Rebuilt with ${errs} error${errs > 1 ? 's' : ''}</button>`
        : `<span class="st-item"><span class="state-dot" style="background:var(--green)"></span>Rebuilt · revision ${d.modelRev} · ${R.ms} ms</span>`;
    }
    const buttons = d ? `<button class="st-btn stop" data-act="stop" ${R.state === 'running' ? '' : 'disabled'} data-tip="Stop the rebuild" data-tipk="Esc">${icon('stop')} Stop</button>
      <button class="st-btn" data-act="rebuild" ${R.state === 'stopped' ? '' : 'disabled'} data-tip="Rebuild" data-tipk="Mod B" data-tipd="Resumes a stopped rebuild">${icon('rebuild')} Rebuild</button>` : '';
    let mid = '';
    if (S.sketch && window.SK) mid = SK.statusText();
    else if (S.hover) { const m = VP.model(); const ref = m && (m.faces.find((f) => f.id === S.hover.id) || m.edges.find((e) => e.id === S.hover.id) || m.verts.find((v) => v.id === S.hover.id)); if (ref) mid = `<span class="st-item">${esc(S.hover.id)} · ${S.hover.kind === 'face' ? (ref.kind === 'plane' ? 'Planar' : 'Cylindrical') : S.hover.kind === 'edge' ? ref.kind : 'vertex'} · ${esc(featName(d, ref.maker))}</span>`; }
    else if (S.sel.length) mid = `<span class="st-item">${S.sel.length} selected</span>`;
    else if (S.pickPlane) mid = '<span class="st-item strong">Pick a plane or a flat face to sketch on</span>';
    let right = '';
    if (d) {
      if (d.disk === 'changed') right += `<button class="st-item warn-text" data-act="showConflict">${icon('warn')} Changed on disk</button><span class="st-sep"></span>`;
      if (d.disk === 'gone') right += `<span class="st-item err-text">${icon('error')} File gone</span><span class="st-sep"></span>`;
      right += `<span class="st-item">${dirty(d) ? `<span class="strong">Unsaved</span> · ${S.dialog ? 'dialog open' : d.undo.length + ' edit' + (d.undo.length === 1 ? '' : 's')}` : 'Saved'}</span><span class="st-sep"></span><span class="st-item">mm</span><span class="st-sep"></span>`;
    }
    right += `<button class="st-item" data-act="shortcuts">${icon('help')} Shortcuts <span class="kbd">?</span></button>`;
    $('#status').innerHTML = `${left}${buttons}<span class="st-sep" style="margin:0 6px"></span>${mid}<span class="st-grow"></span>${right}`;
  }

  // ───────── Canvas chrome: banners, hints, nav bar, view cube buttons ─────────
  function renderCanvasUI() {
    const d = doc(); let html = '';
    if (S.banner) html += renderBanner(d);
    if (!d) {
      html += `<div class="empty-canvas"><div class="t">No document open</div><div class="s">The canvas is ready. Start a part, or open one.</div><div class="acts"><button class="btn btn-primary" data-act="newDoc" style="height:28px">${icon('newDoc')} New part ${kbd(['Mod', 'N'])}</button><button class="btn btn-secondary" data-act="open" style="height:28px">${icon('open')} Open… ${kbd(['Mod', 'O'])}</button></div>
        <div class="s" style="margin-top:14px;font-size:11px">Recent: <span class="link" data-act="openRecent" data-k="bracket">bracket.silico</span> · <span class="link" data-act="openRecent" data-k="plate">plate-with-holes.silico</span></div></div>`;
    } else if (!d.features.length && !S.sketch && !S.pickPlane) {
      html += `<div class="canvas-hint" style="top:auto;bottom:64px">${icon('sketch')}<span>Start with a sketch: <b class="strong" style="font-weight:inherit">Create Sketch</b>, then pick a plane.</span><button class="btn btn-primary" data-act="cmd" data-cmd="createSketch">Create Sketch</button></div>`;
    }
    if (S.pickPlane) html += `<div class="canvas-hint">${icon('plane')}<span>Pick an origin plane or a flat face to sketch on</span><button class="btn btn-ghost" data-act="cancelPick">Cancel <span class="kbd">Esc</span></button></div>`;
    if (S.sketch && window.SK) html += SK.renderCanvasUI();
    const vn = VP.viewName ? VP.viewName() : null;
    html += `<div class="nav-bar" role="toolbar" aria-label="Navigation">
      <button class="ab-btn ${S.navMode === 'orbit' ? 'on' : ''}" data-act="navMode" data-m="orbit" data-tip="Orbit" data-tipk="Shift Middle-drag" data-tipd="Or click here, then drag with the left button. Esc returns to Select. Alt-drag also orbits in this prototype">${icon('orbit')}</button>
      <button class="ab-btn ${S.navMode === 'pan' ? 'on' : ''}" data-act="navMode" data-m="pan" data-tip="Pan" data-tipk="Middle-drag">${icon('pan')}</button>
      <button class="ab-btn" data-act="zoomIn" data-tip="Zoom" data-tipk="Wheel" data-tipd="The wheel zooms toward the pointer">${icon('zoom')}</button>
      <button class="ab-btn" data-act="fit" data-tip="Fit" data-tipk="F6">${icon('fit')}</button>
      <button class="ab-btn" data-act="lookAt" data-tip="Look At" data-tipk="Mod 8" data-tipd="Square to the selected face">${icon('lookAt')}</button>
      <span class="ab-sep"></span>
      <button class="ab-btn" data-act="displayMenu" aria-haspopup="menu" data-tip="Display settings" data-tipd="Visual style, camera, grid">${icon('display')}${icon('down')}</button>
      <button class="ab-btn ${S.grid !== false ? 'on' : ''}" data-act="toggleGrid" data-tip="Grid" aria-pressed="${S.grid !== false}">${icon('grid')}</button>
    </div>
    <div class="view-label" style="left:82px">${VP.cam.persp ? 'Perspective' : 'Orthographic'}${vn ? ' · ' + vn[0].toUpperCase() + vn.slice(1) : ''}</div>
    <div style="position:absolute;right:${16 + 92 + 4}px;top:16px;display:flex;flex-direction:column;gap:2px;z-index:20">
      <button class="ab-btn" data-act="home3d" data-tip="Home view" data-tipk="Home" style="background:var(--g200)">${icon('home')}</button>
      <button class="ab-btn" data-act="cubeMenu" data-tip="View cube options" data-tipd="Orthographic or perspective" style="background:var(--g200)">${icon('down')}</button>
    </div>`;
    $('#canvas-ui').innerHTML = html;
  }
  function renderBanner(d) {
    const B = S.banner; if (!B) return '';
    if (B.kind === 'reloaded') {
      return `<div class="canvas-banner info" role="status" style="right:148px">${icon('diff', 'b-icon')}<div class="b-text"><div class="t">Reloaded ${esc(d.file)}: another program changed it on disk</div>
        <div class="s">Revision ${B.from} → ${B.to}. You had no unsaved edits, so nothing of yours was lost. Changed features are marked <span class="state-dot" style="background:var(--purple)"></span> until you look at them.</div>
        <div class="diff">${B.changes.map((c) => `<span class="k">${esc(c.feature)}</span><span>${esc(c.field)}</span><span>${esc(c.text)}</span>`).join('')}</div></div>
        <div class="b-actions"><button class="btn btn-secondary" data-act="undoReload" data-tip="Puts back the version you had" data-tipd="An edit like any other: Save writes it">Restore my version</button><button class="ab-btn" data-act="dismissBanner" aria-label="Dismiss">${icon('close')}</button></div></div>`;
    }
    if (B.kind === 'conflict') {
      return `<div class="canvas-banner warn" role="alert" style="right:148px">${icon('warn', 'b-icon')}<div class="b-text"><div class="t">${esc(d.file)} changed on disk while you have ${d.undo.length} unsaved edit${d.undo.length === 1 ? '' : 's'}</div>
        <div class="s">Another program wrote the file: an agent through <span class="mono">silico serve</span>, git, or an editor. Your edits are safe here. Saving over the file would lose its change, so Save waits until you choose.</div></div>
        <div class="b-actions"><button class="btn btn-secondary" data-act="showDiff">Show changes</button><button class="btn btn-secondary" data-act="reloadDiscard">Reload, discard my edits</button><button class="btn btn-secondary" data-act="keepMine">Keep my edits…</button></div></div>`;
    }
    if (B.kind === 'gone') {
      return `<div class="canvas-banner err" role="alert" style="right:148px">${icon('error', 'b-icon')}<div class="b-text"><div class="t">${esc(d.file)} was moved or deleted</div><div class="s">The part is still open here, with your edits. Save As writes it somewhere new.</div></div>
        <div class="b-actions"><button class="btn btn-secondary" data-act="saveAsCmd">Save As…</button><button class="btn btn-secondary" data-act="closeTab" data-id="${d.uid}">Close</button></div></div>`;
    }
    return '';
  }

  // ───────── Floating: menus, modals, palette, tooltip ─────────
  function renderFloating() {
    let html = '';
    if (S.menu) {
      const M = S.menu;
      html += `<div class="menu ${M.cls || ''}" role="menu" style="left:${M.x}px;top:${M.y}px" id="menu">${M.items.map((it, i) => {
        if (it === 'sep') return '<div class="msep"></div>';
        if (it.head) return `<div class="mh">${esc(it.head)}</div>`;
        return `<button class="mi ${it.danger ? 'danger' : ''} ${M.kfocus === i ? 'kfocus' : ''}" role="menuitem" data-act="menuItem" data-i="${i}" ${it.disabled ? 'disabled' : ''} ${it.hover ? `data-hover="${i}"` : ''}>${it.icon ? icon(it.icon) : '<span style="width:14px"></span>'}<span>${esc(it.label)}</span>${it.sub ? `<span class="subdued" style="margin-left:8px;font-size:11px">${esc(it.sub)}</span>` : ''}<span class="sc">${kbd(it.keys)}</span></button>`;
      }).join('')}${M.foot ? `<div class="foot">${M.foot}</div>` : ''}</div>`;
    }
    if (S.modal) html += renderModal();
    if (S.palette) html += renderPalette();
    $('#floating').innerHTML = html;
    if (S.menu) {
      const m = $('#menu'); const r = m.getBoundingClientRect();
      if (r.right > innerWidth - 8) m.style.left = Math.max(8, innerWidth - r.width - 8) + 'px';
      if (r.bottom > innerHeight - 8) m.style.top = Math.max(8, S.menu.y - r.height) + 'px';
    }
  }
  function renderModal() {
    const M = S.modal; const d = doc();
    if (M.type === 'shortcuts') return renderShortcuts(M);
    if (M.type === 'new') {
      const name = M.name ?? 'Untitled-1';
      return `<div class="backdrop" data-act="closeModal"><div class="modal" role="dialog" aria-modal="true" aria-label="New part" data-stop="1">
        <div class="m-head">${icon('newDoc')}<span>New part</span><span class="grow"></span><button class="ab-btn" data-act="closeModal" aria-label="Close">${icon('close')}</button></div>
        <div class="m-body"><div class="lbl">Name</div><div class="field"><input data-key="new-name" data-input="newName" value="${esc(name)}" aria-label="Name"><span class="unit">.silico</span></div>
        <div class="lbl">Folder</div><div style="display:flex;gap:6px"><button class="field select" style="flex:1;height:26px" data-act="noop">~/Documents/Silico ${icon('down')}</button><button class="btn btn-secondary" style="height:26px" data-act="noop">Choose…</button></div>
        <div class="lbl">Creates</div><div class="mono" style="color:var(--g750)">~/Documents/Silico/${esc(name)}.silico</div>
        <div class="hint-block" style="margin:12px 0 0">Silico asks where to save first, so the part is a file from the start: agents and git see the same file you do.</div></div>
        <div class="m-foot"><button class="btn btn-secondary" data-act="closeModal">Cancel <span class="kbd">Esc</span></button><button class="btn btn-primary" data-act="createNew">Create <span class="kbd">↵</span></button></div></div></div>`;
    }
    if (M.type === 'whatswrong') {
      return `<div class="backdrop" data-act="closeModal"><div class="modal" style="width:560px" role="dialog" aria-modal="true" aria-label="What's wrong" data-stop="1">
        <div class="m-head">${icon('error')}<span>What's wrong in ${esc(d.file)}</span><span class="grow"></span><button class="ab-btn" data-act="closeModal" aria-label="Close">${icon('close')}</button></div>
        <div class="m-body" style="padding:0 4px 8px">${d.problems.length ? d.problems.map((p) => errCard(p)).join('') : '<div class="hint-block">No problems. Every feature built.</div>'}</div></div></div>`;
    }
    if (M.type === 'overdefine') {
      return `<div class="backdrop"><div class="modal" role="dialog" aria-modal="true" aria-label="Dimension over-defines the sketch" data-stop="1">
        <div class="m-head">${icon('warn')}<span>This dimension over-defines ${esc(M.sketch)}</span></div>
        <div class="m-body"><p style="margin:0 0 12px;color:var(--g750)">The sketch is already fully defined. A ${esc(M.value)} mm length here repeats what the other dimensions give.</p>
        <button class="choice" data-act="odChoice" data-v="driven"><div><div class="c-t">Make it driven</div><div class="c-s">Shows the measured value in parentheses, and constrains nothing. The sketch stays fully defined.</div></div><span class="kbd">1</span></button>
        <button class="choice" data-act="odChoice" data-v="driving"><div><div class="c-t">Leave it driving</div><div class="c-s">The sketch is over defined, drawn red, and Legs and everything after it won't build until you remove a dimension.</div></div><span class="kbd">2</span></button>
        <button class="choice" data-act="odChoice" data-v="cancel"><div><div class="c-t">Cancel</div><div class="c-s">Removes the dimension.</div></div><span class="kbd">Esc</span></button></div></div></div>`;
    }
    if (M.type === 'saveConflict') {
      return `<div class="backdrop" data-act="closeModal"><div class="modal" role="dialog" aria-modal="true" data-stop="1">
        <div class="m-head">${icon('warn')}<span>Can't save over a file that changed</span></div>
        <div class="m-body"><p style="margin:0;color:var(--g750)">${esc(d.file)} changed on disk after you opened it. Saving now would erase that change. Save your version under another name, or reload theirs.</p></div>
        <div class="m-foot"><button class="btn btn-secondary" data-act="closeModal">Cancel</button><button class="btn btn-secondary" data-act="reloadDiscard">Reload, discard my edits</button><button class="btn btn-primary" data-act="saveAsCmd">Save As…</button></div></div></div>`;
    }
    if (M.type === 'saveAs') {
      return `<div class="backdrop" data-act="closeModal"><div class="modal" role="dialog" aria-modal="true" data-stop="1">
        <div class="m-head">${icon('save')}<span>Save As</span></div>
        <div class="m-body"><div class="lbl">Name</div><div class="field"><input data-key="saveas" value="${esc(d.file.replace('.silico', ''))}-mine" aria-label="Name"><span class="unit">.silico</span></div>
        <div class="hint-block" style="margin:12px 0 0">The tab then holds the new file. The original stays as the other program left it.</div></div>
        <div class="m-foot"><button class="btn btn-secondary" data-act="closeModal">Cancel</button><button class="btn btn-primary" data-act="doSaveAs">Save <span class="kbd">↵</span></button></div></div></div>`;
    }
    if (M.type === 'diff') {
      const ch = d.pendingDisk ? d.pendingDisk.changes : [];
      return `<div class="backdrop" data-act="closeModal"><div class="modal" style="width:520px" role="dialog" aria-modal="true" data-stop="1">
        <div class="m-head">${icon('diff')}<span>What changed on disk</span><span class="grow"></span><button class="ab-btn" data-act="closeModal" aria-label="Close">${icon('close')}</button></div>
        <div class="m-body"><div class="section" style="padding-left:0">Their change (on disk)</div>${ch.map((c) => `<div class="prop" style="padding:0"><span class="k" style="color:var(--purple)">${esc(c.feature)}</span><span class="v">${esc(c.field)} · ${esc(c.text)}</span></div>`).join('')}
        <div class="section" style="padding-left:0">Your unsaved edits</div>${d.undo.map((u) => `<div class="prop" style="padding:0"><span class="k">Edit</span><span class="v">${esc(u.label)}</span></div>`).join('')}
        <div class="hint-block" style="margin:12px 0 0">A feature-level summary of the two documents, as <span class="mono">silico textconv</span> reads them for <span class="mono">git diff</span>.</div></div>
        <div class="m-foot"><button class="btn btn-secondary" data-act="closeModal">Close</button><button class="btn btn-secondary" data-act="reloadDiscard">Reload theirs</button><button class="btn btn-primary" data-act="saveAsCmd">Save mine as…</button></div></div></div>`;
    }
    if (M.type === 'keepMine') {
      return `<div class="backdrop" data-act="closeModal"><div class="modal" role="dialog" aria-modal="true" data-stop="1">
        <div class="m-head">${icon('warn')}<span>Keep your edits</span></div>
        <div class="m-body"><p style="margin:0;color:var(--g750)">You keep working on your version. ${esc(d.file)} keeps its new contents on disk, and Save stays off until you reload or Save As. The status bar shows <span class="warn-text">Changed on disk</span> meanwhile.</p></div>
        <div class="m-foot"><button class="btn btn-secondary" data-act="closeModal">Back</button><button class="btn btn-secondary" data-act="keepMineConfirm">Keep working</button><button class="btn btn-primary" data-act="saveAsCmd">Save As now…</button></div></div></div>`;
    }
    if (M.type === 'closeDirty') {
      return `<div class="backdrop" data-act="closeModal"><div class="modal" role="dialog" aria-modal="true" data-stop="1">
        <div class="m-head">${icon('save')}<span>Save ${esc(M.file)} before closing?</span></div>
        <div class="m-body"><p style="margin:0;color:var(--g750)">It has ${M.edits} unsaved edit${M.edits === 1 ? '' : 's'}. Closing without saving drops them.</p></div>
        <div class="m-foot"><button class="btn btn-danger" data-act="closeDiscard" data-id="${M.uid}">Don't save</button><span style="flex:1"></span><button class="btn btn-secondary" data-act="closeModal">Cancel</button><button class="btn btn-primary" data-act="closeSave" data-id="${M.uid}">Save <span class="kbd">↵</span></button></div></div></div>`;
    }
    return '';
  }
  function renderShortcuts(M) {
    const q = (M.q || '').toLowerCase();
    const groups = {};
    for (const [id, c] of Object.entries(CMD)) { if (!c.keys || !c.keys.length) continue; (groups[c.group] = groups[c.group] || []).push(c); }
    const order = ['File', 'Edit', 'Rebuild', 'View', 'Selection', 'Dialogs', 'Sketch', 'Help'];
    const notes = { Sketch: 'while a sketch is open', Selection: 'outside sketches', View: 'Fusion’s mouse' };
    const hl = (t) => q ? esc(t).replace(new RegExp('(' + q.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + ')', 'ig'), '<mark>$1</mark>') : esc(t);
    const cols = order.map((g) => {
      const items = (groups[g] || []).filter((c) => !q || c.label.toLowerCase().includes(q) || (c.desc || '').toLowerCase().includes(q));
      if (!items.length) return '';
      return `<div class="grp"><div class="grp-t"><span>${g}</span><span style="text-transform:none;letter-spacing:0">${notes[g] || ''}</span></div>${items.map((c) => `<div class="sc-row"><span class="d">${hl(c.label)}</span><span class="keys">${kbd(c.keys)}</span></div>`).join('')}</div>`;
    }).join('');
    return `<div class="backdrop" data-act="closeModal"><div class="modal sheet" role="dialog" aria-modal="true" aria-label="Keyboard shortcuts" data-stop="1">
      <div class="m-head">${icon('help')}<span>Keyboard shortcuts</span><span class="grow"></span>
        <div class="field" style="width:220px;height:24px">${icon('search')}<input data-key="sc-q" data-input="scq" placeholder="Filter" value="${esc(M.q || '')}" aria-label="Filter shortcuts"></div>
        <button class="ab-btn" data-act="closeModal" aria-label="Close">${icon('close')}</button></div>
      <div class="m-body"><div class="cols">${cols || '<div class="note">No shortcut matches.</div>'}</div>
      <div class="note" style="margin-top:6px;border-top:1px solid var(--g250);padding-top:8px"><b class="strong" style="font-weight:inherit">Esc does the nearest thing first:</b> ends a drag, then a sketch tool's chain, then stops a dialog's preview, then cancels the dialog, then clears the selection, then stops the rebuild.
      ${MAC ? '' : ''} In this browser prototype, ${KEYNAME.Mod}N, ${KEYNAME.Mod}W, and ${KEYNAME.Mod}1–8 belong to the browser; use the menus and the view cube.</div></div></div></div>`;
  }
  function paletteItems() {
    const q = (S.palette.q || '').toLowerCase();
    return Object.entries(CMD).filter(([id, c]) => !['ok', 'cancel', 'orbit', 'pan', 'zoom', 'sNoRel', 'selectOther'].includes(id))
      .filter(([id, c]) => !q || c.label.toLowerCase().includes(q) || c.group.toLowerCase().includes(q) || (c.desc || '').toLowerCase().includes(q))
      .slice(0, 40);
  }
  function renderPalette() {
    const items = paletteItems(); const idx = Math.min(S.palette.idx || 0, Math.max(0, items.length - 1));
    return `<div class="backdrop" style="background:rgba(0,0,0,.35)" data-act="closePalette"></div><div class="palette-modal" role="dialog" aria-label="Search commands">
      <div class="pm-input">${icon('search')}<input data-key="pal-q" data-input="palq" placeholder="Search commands" value="${esc(S.palette.q || '')}" aria-label="Search commands" autocomplete="off"></div>
      <div class="pm-list menu" style="position:static;box-shadow:none;border:0;background:transparent" role="listbox">${items.map(([id, c], i) => `<button class="mi ${i === idx ? 'kfocus' : ''}" data-act="runPal" data-cmd="${id}" role="option" aria-selected="${i === idx}">${icon(c.icon || 'chevron')}<span>${esc(c.label)}</span><span class="subdued" style="margin-left:8px;font-size:11px">${c.group}</span><span class="sc">${kbd(c.keys)}</span></button>`).join('') || '<div class="mh">No command matches</div>'}</div>
      <div class="pm-foot"><span>${kbd(['↑'])}${kbd(['↓'])} move</span><span>${kbd(['Enter'])} run</span><span>${kbd(['Esc'])} close</span><span style="margin-left:auto">Agents reach the same commands over MCP</span></div></div>`;
  }

  // ───────── Start page ─────────
  const THUMBS = {};
  function thumb(kind) { if (!THUMBS[kind]) { try { THUMBS[kind] = VP.thumb(kind, 260, 160); } catch (e) { THUMBS[kind] = ''; } } return THUMBS[kind]; }
  function renderStart() {
    const recents = [
      { k: 'bracket', name: 'bracket.silico', path: '~/Desktop/silico-samples', when: '12 min ago' },
      { k: 'plate', name: 'plate-with-holes.silico', path: '~/Desktop/silico-samples', when: 'Yesterday' },
      { k: 'chamfer', name: 'chamfered-box.silico', path: '~/Desktop/silico-samples', when: 'Oct 28' },
      { k: 'missing', name: 'vise-jaw.silico', path: '~/Git/fixtures', when: 'Oct 21', missing: true },
    ];
    $('#start').innerHTML = `<div class="st-wrap">
      <div>
        <div class="st-brand"><svg class="i logo" viewBox="0 0 20 20" style="stroke-width:1.4">${ICONS.logo}</svg>Silico</div>
        <div class="st-actions">
          <button class="st-action primary" data-act="newDoc">${icon('newDoc')}<span>New part</span>${kbd(['Mod', 'N'])}</button>
          <button class="st-action" data-act="open">${icon('open')}<span>Open…</span>${kbd(['Mod', 'O'])}</button>
          <button class="st-action" data-act="search">${icon('search')}<span>Search commands</span><span class="kbd">S</span></button>
          <button class="st-action" data-act="shortcuts">${icon('help')}<span>Keyboard shortcuts</span><span class="kbd">?</span></button>
        </div>
        <div class="st-side-note">Parts are plain <span class="mono">.silico</span> files. Agents edit the same files through <span class="mono">silico serve</span> (MCP). When one changes a part you have open, Silico reloads it and marks what changed.
          <div style="margin-top:10px"><button class="btn btn-ghost" style="padding:0 6px;margin-left:-6px" data-act="copy" data-text='{"mcpServers":{"silico":{"command":"silico","args":["serve"]}}}'>${icon('copy')} Copy MCP config</button></div></div>
      </div>
      <div>
        <div class="st-h"><span>Recent</span><span class="grow"></span><span style="text-transform:none;letter-spacing:0">${kbd(['↑'])}${kbd(['↓'])} ${kbd(['Enter'])} opens</span></div>
        <div class="recent" role="list">${recents.map((r, i) => `<button class="recent-row ${r.missing ? 'missing' : ''}" role="listitem" data-act="openRecent" data-k="${r.k}" data-ri="${i}">
          <span class="thumb">${r.missing ? icon('warn') : `<img alt="" src="${thumb(r.k === 'chamfer' ? 'chamfer' : r.k)}">`}</span>
          <span><span class="nm">${esc(r.name)}</span><br><span class="pth mono">${esc(r.path)}</span>${r.missing ? ' <span class="err-text" style="font-size:11px">· not found</span>' : ''}</span>
          <span class="when">${r.missing ? '<span class="link" data-act="removeRecent">Remove from list</span>' : r.when}</span></button>`).join('')}</div>
        <div class="st-h"><span>Samples</span><span class="grow"></span><span style="text-transform:none;letter-spacing:0">Opened as copies, so the originals stay clean</span></div>
        <div class="samples">
          ${[['bracket', 'Bracket', 'Sketch, pad, two pockets, a fillet. Seven features.'], ['plate', 'Plate with holes', 'A box with a hole pattern.'], ['chamfer', 'Chamfered box', 'A box and a chamfer on every edge.']].map(([k, n, ds]) => `<button class="sample" data-act="openRecent" data-k="${k}"><div class="sthumb"><img alt="" src="${thumb(k)}"></div><div class="smeta"><div class="nm">${n}</div><div class="ds">${ds}</div></div></button>`).join('')}
        </div>
        <div class="drop-note">Drop a <span class="mono">.silico</span> file anywhere in this window to open it.</div>
      </div></div>`;
  }

  // ───────── Prototype strip (not part of the app) ─────────
  const PROTO = [
    ['1', 'Start'], ['1b', 'Blank canvas'], ['2', 'Part open'], ['2r', 'Rebuilding'], ['3', 'Pad dialog'], ['3e', 'Failed preview'], ['4', 'Selection'],
    ['5', 'Sketch: dimension'], ['5n', 'New sketch'], ['6a', 'Failed feature'], ['6b', 'Broken reference'], ['6c', 'Agent edit'], ['6d', 'Agent edit + your edits'], ['7', 'Shortcuts'],
  ];
  function renderProto() {
    const P = S.proto; const cur = PROTO.find((x) => x[0] === P.current);
    $('#proto').innerHTML = `<button class="proto-toggle" data-act="protoToggle" aria-expanded="${!P.collapsed}" aria-label="Prototype states">Prototype · ${cur ? cur[0] + ' ' + cur[1] : 'states'} ${P.collapsed ? '▾' : '▴'}</button>
      ${P.collapsed ? '' : `<div class="proto-list">${PROTO.map(([k, l]) => `<button class="${P.current === k ? 'on' : ''}" data-act="proto" data-k="${k}"><span class="pk">${k}</span>${l}</button>`).join('')}
      <div class="sep"></div><button class="${S.slow ? 'on' : ''}" data-act="protoSlow" title="Each rebuild step takes 650 ms, so Stop can be tried"><span class="pk">${S.slow ? '✓' : ''}</span>Slow kernel</button>
      <div class="proto-note">Not part of the app: jumps between the states under review.</div></div>`}`;
  }

  // Expose for sketch.js and interactions.
  window.APP = { S, CMD, KEYNAME, kbd, esc, fmt, fmtF, doc, render, renderStatus, renderTimeline, renderRight, renderCanvasUI, renderFloating, toast, edit, undo, redo, save, setSel, toggleSel, syncHighlight,
    startRebuild, stopRebuild, resumeRebuild, publish, openDialog, okDialog, cancelDialog, dialogInput, makeDoc, evaluate, snap, restore, dirty, featName, paletteItems, CHILDREN, FEATURE_KIND, MAC, parseValue, renderBrowser };
  APP.syncSketchLines = () => syncSketchLines();
  function syncSketchLines() {
    const d = doc(); if (!d) { VP.setSketchLines([]); return; }
    const lines = [];
    const p = d.p;
    const V = (x, y, z) => new THREE.Vector3(x, y, z);
    const circ = (c, U, Vv, r) => { const pts = []; for (let i = 0; i <= 48; i++) { const t = i / 48 * Math.PI * 2; pts.push(c.clone().addScaledVector(U, Math.cos(t) * r).addScaledVector(Vv, Math.sin(t) * r)); } return pts; };
    if (S.sketch) { VP.setSketchLines([]); return; }
    for (const f of d.features) {
      if (!S.vis.sketches[f.id]) continue;
      const col = d.sketchState[f.id] === 'under' ? '#7a95ff' : '#eeeeed';
      if (f.id === 's0000001') lines.push({ pts: [V(0, 0, 0), V(p.W, 0, 0), V(p.W, 0, p.tb), V(p.tu, 0, p.tb), V(p.tu, 0, p.H), V(0, 0, p.H)], color: col, closed: true });
      if (f.id === 's0000002') [12, 28].forEach((y) => lines.push({ pts: circ(V(40, y, 0), V(1, 0, 0), V(0, 1, 0), 2.75), color: col }));
      if (f.id === 's0000003') [12, 28].forEach((y) => lines.push({ pts: circ(V(0, y, 35), V(0, 1, 0), V(0, 0, 1), 2.75), color: col }));
      const ns = d.newSketches[f.id];
      if (ns) { const fr = { o: new THREE.Vector3(...ns.frame.o), U: new THREE.Vector3(...ns.frame.U), V: new THREE.Vector3(...ns.frame.V) }; ns.lines.forEach((l) => { const a = ns.points[l.a], b = ns.points[l.b]; lines.push({ pts: [fr.o.clone().addScaledVector(fr.U, a.u).addScaledVector(fr.V, a.v), fr.o.clone().addScaledVector(fr.U, b.u).addScaledVector(fr.V, b.v)], color: '#7a95ff' }); }); }
    }
    VP.setSketchLines(lines);
  }
})();
