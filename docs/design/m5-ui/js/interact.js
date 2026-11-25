/* Interaction: actions, keyboard, viewport hooks, menus, and the prototype's states. */
(function () {
  const A = window.APP; const S = A.S; const $ = (s) => document.querySelector(s);
  const doc = A.doc;

  // ───────── Opening and closing documents ─────────
  function openBracket(fresh) {
    let d = S.docs.find((x) => x.kind === 'bracket');
    if (d && fresh) { S.docs = S.docs.filter((x) => x !== d); d = null; }
    if (!d) { d = A.makeDoc('bracket', 'bracket.silico', '~/Desktop/silico-samples/bracket.silico'); S.docs.push(d); }
    activate(d);
    return d;
  }
  function activate(d) {
    if (S.sketch) SK.cancel();
    S.active = d.uid; S.view = 'doc'; S.sel = []; S.hover = null; S.selFeature = null; S.selRow = null; S.dialog = null; S.banner = null; S.pickPlane = null;
    S.rebuild = { state: 'done', done: d.features.length, steps: d.features.length, ms: d.kind === 'bracket' ? 84 : 0 };
    A.render();
    A.publish(d); VP.fitBox(VP.modelBox(), VP.VIEWS.iso); A.render();
  }
  function closeDoc(uid, force) {
    const d = S.docs.find((x) => x.uid === uid); if (!d) return;
    if (!force && A.dirty(d)) { S.modal = { type: 'closeDirty', uid, file: d.file, edits: d.undo.length }; A.render(); return; }
    if (S.sketch && S.active === uid) SK.cancel();
    S.docs = S.docs.filter((x) => x.uid !== uid);
    if (S.active === uid) { const n = S.docs[S.docs.length - 1]; if (n) activate(n); else { S.active = null; S.view = 'start'; VP.setModel(null); } }
    S.modal = null; A.render();
  }
  // Untitled parts are named "Untitled", then "Untitled 2", "Untitled 3", from one counter
  // never reused in a run (macOS's rule).
  let untitledSeq = 0;
  function newDoc() {
    untitledSeq++;
    const name = untitledSeq === 1 ? 'Untitled' : 'Untitled ' + untitledSeq;
    const d = A.makeDoc('empty', name, 'Not saved yet');
    d.untitled = true;
    S.docs.push(d); S.modal = null; activate(d);
    VP.fitBox(VP.modelBox(), VP.VIEWS.iso);
  }

  // ───────── Commands ─────────
  function runCmd(id) {
    const d = doc();
    switch (id) {
      case 'newDoc': newDoc(); return;
      case 'open': A.toast("The system's Open dialog appears here. This prototype opens bracket.silico."); openBracket(); return;
      case 'save': A.save(); return;
      case 'saveAs': if (d) { S.modal = { type: 'saveAs' }; A.render(); } return;
      case 'revert': if (d && A.dirty(d)) { A.restore(d, d.saved); d.undo = []; d.redo = []; d.rev++; A.startRebuild(d); A.toast('Reverted to the saved file'); } return;
      case 'close': if (d) closeDoc(d.uid); return;
      case 'undo': A.undo(); return;
      case 'redo': A.redo(); return;
      case 'search': S.palette = { q: '', idx: 0 }; A.render(); setTimeout(() => { const i = $('[data-key="pal-q"]'); i && i.focus(); }, 0); return;
      case 'shortcuts': S.modal = { type: 'shortcuts', q: '' }; A.render(); setTimeout(() => { const i = $('[data-key="sc-q"]'); i && i.focus(); }, 0); return;
      case 'rebuild': if (d) A.resumeRebuild(); return;
      case 'rebuildAll': if (d) { A.startRebuild(d); A.toast('Rebuilding every feature, ignoring the cache'); } return;
      // Trevin's ask of 2026-11-09: the file read again, then every feature rebuilt. A gone file drops
      // nothing (Save As keeps the work); unsaved edits are asked about first, as Close asks.
      case 'reload':
        if (!d) return;
        if (S.dialog) { A.toast('Finish or cancel the dialog first: Reload drops what it holds.'); return; }
        if (d.disk === 'gone') { A.toast(`${d.file} is gone from disk: nothing to reload. Save As keeps your work`); return; }
        if (A.dirty(d) && !d.untitled) { S.modal = { type: 'reloadDirty', file: d.file, edits: d.undo.length }; A.render(); return; }
        A.startRebuild(d); A.toast(d.untitled ? 'Rebuilding every feature' : 'Reloaded from disk, rebuilding every feature'); return;
      case 'stop': A.stopRebuild(); return;
      case 'whatsWrong': if (d) { S.modal = { type: 'whatswrong' }; A.syncHighlight(); A.render(); } return;
      case 'createSketch': if (!d) { runCmd('newDoc'); return; } if (S.sketch) return; startPickPlane(); return;
      case 'pad': if (d) A.openDialog('pad', null); return;
      case 'pocket': case 'revolve': case 'groove': case 'chamfer': case 'union': case 'subtract': case 'intersect':
        A.toast(A.CMD[id].label + ' opens its dialog here, docked at the right, like Pad.'); return;
      case 'fillet': if (d && d.kind === 'bracket') A.openDialog('fillet', 'f0000001'); return;
      case 'rectPattern': PAT.open('linear_pattern', null); return;
      case 'circPattern': PAT.open('polar_pattern', null); return;
      case 'mirror': PAT.open('mirror', null); return;
      case 'measure': if (S.sketch) { SK.measure(); return; } A.toast('Measure: select two faces. The Properties panel shows their distance or angle.'); return;
      case 'filterFace': toggleFilter('face'); return;
      case 'filterEdge': toggleFilter('edge'); return;
      case 'filterVertex': toggleFilter('vertex'); return;
      case 'clearSel': A.setSel([]); return;
      case 'hide': S.vis.body = !S.vis.body; VP.setBodyVisible(S.vis.body); A.render(); A.toast(S.vis.body ? 'Body1 shown' : 'Body1 hidden. H shows it again.'); return;
      case 'fit': VP.fit(); return;
      case 'home': VP.home(); return;
      case 'vFront': VP.view('front'); return; case 'vBack': VP.view('back'); return; case 'vLeft': VP.view('left'); return; case 'vRight': VP.view('right'); return;
      case 'vTop': VP.view('top'); return; case 'vBottom': VP.view('bottom'); return; case 'vIso': VP.view('iso'); return;
      case 'lookAt': lookAt(); return;
      case 'sLine': SK.setTool('line'); return; case 'sRect': SK.setTool('rect'); return; case 'sCircle': SK.setTool('circle'); return; case 'sArc': SK.setTool('arc'); return;
      case 'sPoint': SK.setTool('point'); return; case 'sTrim': SK.setTool('trim'); return; case 'sDim': SK.setTool('dimension'); return;
      case 'sExtend': case 'sSplit': SK.setTool(id); return;
      case 'sConstruction': A.toast('Construction: select curves first, then X toggles them.'); return;
      case 'finishSketch': SK.finish(); return;
      case 'rename': renameSelected(); return;
      case 'del': A.toast('Delete removes the selected feature, rewiring what follows it (remove_feature).'); return;
      default: A.toast(A.CMD[id] ? A.CMD[id].label : id);
    }
  }
  function toggleFilter(k) {
    if (S.sketch) return;
    S.filter[k] = !S.filter[k]; VP.filter = S.filter;
    if (!S.filter.face && !S.filter.edge && !S.filter.vertex) A.toast('Every filter is off: nothing in the canvas can be selected. X, E, or V turns one back on.', 'warn');
    A.render();
  }
  function lookAt() {
    if (S.sketch) { SK.lookAt(); return; }
    const m = VP.model(); const f = m && S.sel.length === 1 && S.sel[0].kind === 'face' && m.faces.find((x) => x.id === S.sel[0].id);
    if (!f || f.kind !== 'plane') { A.toast('Look At: select a flat face first.'); return; }
    const N = new THREE.Vector3(...f.normal);
    VP.fitBox(VP.modelBox(), { az: Math.abs(N.z) > 0.999 ? 0 : Math.atan2(N.x, -N.y), el: Math.asin(N.z) });
  }
  function renameSelected() {
    const d = doc(); if (!d || !S.selFeature) { A.toast('Select a feature to rename (F2)'); return; }
    const f = d.features.find((x) => x.id === S.selFeature);
    const n = prompt('Rename ' + f.name, f.name);
    if (n && n.trim() && n !== f.name) A.edit((dd) => { dd.features.find((x) => x.id === f.id).name = n.trim(); }, { label: 'Rename ' + f.name });
  }
  function startPickPlane() {
    S.pickPlane = { hover: null }; S.sel = [];
    VP.setPlanes({ XY: true, XZ: true, YZ: true }, null);
    VP.mode = 'pickPlane';
    A.render();
  }
  function endPickPlane() {
    S.pickPlane = null; VP.mode = 'select';
    VP.setPlanes(S.vis.planes, null); A.syncHighlight(); A.render();
  }
  function sketchOn(target) {
    endPickPlane();
    if (target.plane) SK.enter({ plane: target.plane, name: 'Sketch' + ((doc().features.filter((f) => f.type === 'sketch').length) + 1) });
    else {
      const f = target.face; const fr = f.frame; let U = fr.U.clone(), V = fr.V.clone();
      const N = new THREE.Vector3().crossVectors(U, V); const camDir = VP.camBasis().dir;
      if (N.dot(camDir) < 0) U.negate();
      SK.enter({ plane: f.id, frame: { o: fr.o.toArray(), U: U.toArray(), V: V.toArray() }, name: 'Sketch on ' + f.id });
    }
  }

  // ───────── Agents' edits arriving through the file ─────────
  const AGENT = {
    radius: { apply: (d) => { d.p.r = 3; }, list: [{ feature: 'Inside corner', field: 'radius', text: '5 mm → 3 mm' }], marks: { f0000001: 'radius 5 mm → 3 mm' } },
    filletFail: { apply: (d) => { d.p.r = 30; }, list: [{ feature: 'Inside corner', field: 'radius', text: '5 mm → 30 mm' }], marks: { f0000001: 'radius 5 mm → 30 mm' } },
    brokenRef: { apply: (d) => { d.scenario = 'brokenRef'; }, list: [{ feature: 'Bracket profile', field: 'lines', text: 'na000004 split at z = 20 (adds na000007)' }], marks: { s0000001: 'line na000004 split in two at z = 20' } },
  };
  function agentEdit(kind) {
    const d = doc(); if (!d) return; const c = AGENT[kind];
    if (A.dirty(d)) {
      d.disk = 'changed'; d.pendingDisk = { kind, changes: c.list }; S.banner = { kind: 'conflict' }; A.render(); return;
    }
    applyAgent(d, kind);
  }
  function applyAgent(d, kind) {
    const c = AGENT[kind]; const from = d.rev; const prev = A.snap(d);
    c.apply(d); d.rev++; d.saved = A.snap(d); d.undo = []; d.redo = []; d.disk = 'unchanged'; d.pendingDisk = null;
    Object.assign(d.outside, c.marks);
    S.banner = { kind: 'reloaded', from, to: d.rev, changes: c.list, prev };
    A.startRebuild(d);
  }

  // ───────── Event delegation (every clickable thing names its action) ─────────
  const ACT = {
    noop() {},
    home() { if (S.sketch) SK.cancel(); S.view = 'start'; A.render(); },
    tab(el) { const d = S.docs.find((x) => x.uid === el.dataset.id); if (d && (d.uid !== S.active || S.view !== 'doc')) activate(d); },
    closeTab(el, e) { e.stopPropagation(); closeDoc(el.dataset.id || (doc() && doc().uid)); },
    cmd(el) { runCmd(el.dataset.cmd); },
    tool(el) { runCmd(el.dataset.cmd); },
    filter(el) { runCmd(el.dataset.cmd); },
    save() { runCmd('save'); }, undo() { runCmd('undo'); }, redo() { runCmd('redo'); },
    newDoc() { runCmd('newDoc'); }, open() { runCmd('open'); }, search() { runCmd('search'); }, shortcuts() { runCmd('shortcuts'); },
    whatsWrong() { runCmd('whatsWrong'); }, stop() { runCmd('stop'); }, rebuild() { runCmd('rebuild'); },
    fileMenu(el) {
      const d = doc(); const r = el.getBoundingClientRect();
      menu(r.left, r.bottom + 2, [
        { label: 'New part', icon: 'newDoc', keys: ['Mod', 'N'], run: () => runCmd('newDoc') },
        { label: 'Open…', icon: 'open', keys: ['Mod', 'O'], run: () => runCmd('open') },
        { label: 'Open recent', sub: 'bracket.silico', run: () => openBracket() },
        'sep',
        { label: 'Save', icon: 'save', keys: ['Mod', 'S'], disabled: !d, run: () => runCmd('save') },
        { label: 'Save As…', keys: ['Mod', 'Shift', 'S'], disabled: !d, run: () => runCmd('saveAs') },
        { label: 'Revert to saved', disabled: !d || !A.dirty(d), run: () => runCmd('revert') },
        { label: 'Reload', icon: 'rebuild', keys: ['Mod', 'R'], disabled: !d || !!S.dialog, run: () => runCmd('reload') },
        'sep', { head: 'Export' },
        { label: '3MF (printable mesh)…', disabled: !d || !VP.model(), run: () => A.toast('Export runs off the UI thread; its own dialog shows progress and Stop.') },
        { label: 'STEP…', disabled: !d || !VP.model(), run: () => A.toast('Export runs off the UI thread; its own dialog shows progress and Stop.') },
        'sep',
        { label: 'Close', keys: ['Mod', 'W'], disabled: !d, run: () => runCmd('close') },
      ]);
    },
    // Trevin's decision of 2026-11-09: solids are made sketch-first, as SolidWorks makes them, so
    // CREATE holds no primitives; a primitive a file holds is edited (editFeature), never made here.
    // Fusion's CREATE holds Pattern › Rectangular Pattern and Circular Pattern, and Mirror (m6.0 section 7), drawn
    // as MODIFY's Combine group is: a group head, its commands under it.
    createMenu(el) { createMenu(el.getBoundingClientRect()); },
    modifyMenu(el) { const r = el.getBoundingClientRect(); menu(r.left, r.bottom + 2, ['fillet', 'chamfer', 'sep', { head: 'Combine' }, 'union', 'subtract', 'intersect'].map(cmdItem)); },
    selectMenu(el) {
      const r = el.getBoundingClientRect();
      menu(r.left, r.bottom + 2, [{ head: 'Selection filters' },
        { label: (S.filter.face ? '✓ ' : '   ') + 'Faces', icon: 'selFace', keys: ['X'], run: () => toggleFilter('face') },
        { label: (S.filter.edge ? '✓ ' : '   ') + 'Edges', icon: 'selEdge', keys: ['E'], run: () => toggleFilter('edge') },
        { label: (S.filter.vertex ? '✓ ' : '   ') + 'Vertices', icon: 'selVertex', keys: ['V'], run: () => toggleFilter('vertex') },
        'sep', { label: 'Select all filters', run: () => { S.filter = { face: true, edge: true, vertex: true }; VP.filter = S.filter; A.render(); } },
        { label: 'Clear selection', keys: ['Esc'], run: () => A.setSel([]) }]);
    },
    row(el) { if (S.dialog && S.dialog.pattern && /^o-[XYZ]$/.test(el.dataset.id)) { PAT.pick({ word: el.dataset.id.slice(2).toLowerCase() }); return; } S.selRow = el.dataset.id; S.sel = []; S.selFeature = null; A.syncHighlight(); A.render(); },
    rootRow() { const d = doc(); if (d && d.problems.length) runCmd('whatsWrong'); else { S.selRow = 'root'; S.sel = []; S.selFeature = null; A.render(); } },
    planeRow(el) { if (S.dialog && S.dialog.pattern) { PAT.pick({ word: el.dataset.id.slice(3).toLowerCase() }); return; } const pl = el.dataset.id.slice(3); S.selRow = S.selRow === el.dataset.id ? null : el.dataset.id; VP.setPlanes({ ...S.vis.planes, [pl]: S.vis.planes[pl] || S.selRow === el.dataset.id }, S.selRow ? pl : null); A.render(); },
    bodyRow() { S.selRow = 'body1'; S.sel = []; S.selFeature = null; VP.setHighlight({ maker: '*', sel: [], hover: null }); A.render(); },
    featureRow(el) { if (S.dialog && S.dialog.pattern) { PAT.pick({ feature: el.dataset.feature }); return; } selectFeature(el.dataset.feature); },
    selectFeature(el) { selectFeature(el.dataset.id); },
    expand(el, e) { e.stopPropagation(); S.expanded[el.dataset.id] = !S.expanded[el.dataset.id]; A.renderBrowser(); },
    eye(el, e) {
      e.stopPropagation(); const id = el.dataset.id;
      if (id === 'body1') { S.vis.body = !S.vis.body; VP.setBodyVisible(S.vis.body); }
      else if (id === 'origin') { const on = !Object.values(S.vis.planes).some(Boolean); S.vis.planes = { XY: on, XZ: on, YZ: on }; VP.setPlanes(S.vis.planes); }
      else if (id.startsWith('pl-')) { const k = id.slice(3); S.vis.planes[k] = !S.vis.planes[k]; VP.setPlanes(S.vis.planes); }
      else if (id.startsWith('sk-')) { const k = id.slice(3); S.vis.sketches[k] = !S.vis.sketches[k]; A.syncSketchLines(); }
      A.render();
    },
    toggleBrowser() { $('#main').classList.toggle('browser-collapsed'); A.render(); },
    // Trevin's ask of 2026-11-09: ⌘/Ctrl-click on the selected item takes it out; a click on the track's
    // empty space clears the selection (tlTrack); neither while a dialog's boxes hold the selection.
    tlItem(el, e) {
      if (S.dialog && S.dialog.pattern) { PAT.pick({ feature: el.dataset.id }); return; }
      if (e && isMod(e) && S.selFeature === el.dataset.id && !S.dialog) { S.selFeature = null; A.syncHighlight(); A.render(); return; }
      selectFeature(el.dataset.id);
    },
    tlTrack(el, e) { if (e.target !== el || S.dialog) return; S.selFeature = null; S.sel = []; S.selRow = null; A.syncHighlight(); A.render(); },
    editFeature(el) { editFeature(el.dataset.id); },
    showFeature(el) { S.modal = null; selectFeature(el.dataset.id); },
    suppress(el) { const id = el.dataset.id; A.edit((d) => { d.suppressed[id] = !d.suppressed[id]; }, { label: (doc().suppressed[id] ? 'Unsuppress ' : 'Suppress ') + A.featName(doc(), id) }); },
    copy(el) { const t = el.dataset.text; (navigator.clipboard ? navigator.clipboard.writeText(t) : Promise.reject()).then(() => A.toast('Copied'), () => A.toast('Copied (clipboard blocked in this browser): ' + t.slice(0, 60))); },
    useCand(el, e) { e.stopPropagation(); const c = el.dataset.cand; A.edit((d) => { d.scenario = null; d.p.tu = d.p.tu; }, { label: 'Inside corner: use the ' + (c === 'corner' ? 'inside corner' : 'upper') + ' edge' }); S.candHot = null; A.toast('Inside corner now rounds the edge you picked. Undo puts the broken reference back.', 'check'); },
    removeRef() { A.edit((d) => { d.scenario = null; d.suppressed.f0000001 = true; }, { label: 'Inside corner: remove its edge' }); A.toast('Inside corner has no edge now, so it is suppressed until you give it one.'); },
    ackOutside(el) { const d = doc(); delete d.outside[el.dataset.id]; A.render(); },
    navMode(el) { const m = el.dataset.m; S.navMode = S.navMode === m ? null : m; VP.navMode = S.navMode; A.render(); },
    zoomIn() { VP.zoomBy(0.8); }, fit() { VP.fit(); }, lookAt() { lookAt(); }, home3d() { VP.home(); },
    toggleGrid() { S.grid = S.grid === false; VP.setGridVisible(S.grid !== false && !S.sketch); A.render(); },
    displayMenu(el) { const r = el.getBoundingClientRect(); displayMenu(r.left, r.top - 4, true); },
    cubeMenu(el) { const r = el.getBoundingClientRect(); cubeMenu(r.left - 150, r.bottom + 2); },
    dismissBanner() { S.banner = null; A.render(); },
    undoReload() { const B = S.banner; if (!B || !B.prev) return; const prev = B.prev; S.banner = null; A.edit((d) => { A.restore(d, prev); }, { label: 'Restore my version' }); A.toast('Your version is back as an unsaved edit. Save writes it over theirs.'); },
    showDiff() { S.modal = { type: 'diff' }; A.render(); },
    showConflict() { S.banner = { kind: 'conflict' }; A.render(); },
    reloadDirtyDiscard() { const d = doc(); S.modal = null; A.restore(d, d.saved); d.undo = []; d.redo = []; d.rev++; A.startRebuild(d); A.toast('Reloaded from disk, rebuilding every feature'); },
    reloadDiscard() { const d = doc(); const k = d.pendingDisk && d.pendingDisk.kind; S.modal = null; A.restore(d, d.saved); d.undo = []; d.redo = []; if (k) applyAgent(d, k); else { d.disk = 'unchanged'; S.banner = null; A.startRebuild(d); } },
    keepMine() { S.modal = { type: 'keepMine' }; A.render(); },
    keepMineConfirm() { S.modal = null; S.banner = null; A.render(); A.toast('Working on your version. Save is held until you reload or Save As.', 'warn'); },
    saveAsCmd() { S.modal = { type: 'saveAs' }; A.render(); setTimeout(() => { const i = $('[data-key="saveas"]'); i && (i.focus(), i.select()); }, 0); },
    doSaveAs() { const d = doc(); const n = ($('[data-key="saveas"]').value || 'copy').trim(); d.file = n + '.silico'; d.path = d.untitled ? '~/Documents/Silico/' + d.file : d.path.replace(/[^/]+$/, d.file); d.untitled = false; d.saved = A.snap(d); d.undo = []; d.redo = []; d.disk = 'unchanged'; d.pendingDisk = null; S.banner = null; S.modal = null; A.render(); A.toast('Saved as ' + d.path + '. The tab now holds that file.', 'check'); },
    closeModal(el, e) { if (e.target.closest('[data-stop]') && !el.closest('[data-stop]')) return; if (S.modal && S.modal.type === 'overdefine') return; S.modal = null; A.syncHighlight(); A.render(); },
    closeDiscard(el) { closeDoc(el.dataset.id, true); },
    closeSave(el) { const d = S.docs.find((x) => x.uid === el.dataset.id); d.saved = A.snap(d); d.undo = []; closeDoc(d.uid, true); A.toast('Saved and closed ' + d.file); },
    odChoice(el) { SK.overdefineChoice(el.dataset.v); },
    menuItem(el) { const it = S.menu.items[+el.dataset.i]; closeMenu(); it.run && it.run(); },
    closePalette() { S.palette = null; A.render(); },
    runPal(el) { S.palette = null; A.render(); runCmd(el.dataset.cmd); },
    openRecent(el, e) {
      const k = el.dataset.k;
      if (e.target.closest('[data-act="removeRecent"]')) { A.toast('Removed vise-jaw.silico from Recent'); el.remove(); return; }
      if (k === 'bracket') openBracket();
      else if (k === 'plate') PAT.openPlate();
      else if (k === 'missing') A.toast("vise-jaw.silico isn't at ~/Git/fixtures any more. Find it with Open…, or remove it from the list.", 'warn', 'err');
      else A.toast('This prototype models bracket.silico only. ' + (k === 'plate' ? 'plate-with-holes' : 'chamfered-box') + '.silico would open here.');
    },
    removeRecent() {},
    protoToggle() { S.proto.collapsed = !S.proto.collapsed; A.render(); },
    proto(el) { S.proto.collapsed = true; proto(el.dataset.k); },
    protoSlow() { S.slow = !S.slow; A.render(); },
    activateBox(el, e) { if (e.target.closest('[data-act="patClear"],[data-act="patRemoveObj"]')) return; if (S.dialog) { S.dialog.activeBox = el.dataset.box; A.render(); } },
    patDrop(el) { PAT.dropMenu(el, el.dataset.k); },
    patFlip(el) { PAT.flip(el.dataset.k); },
    patClear(el, e) { e.stopPropagation(); PAT.clearBox(el.dataset.k); },
    patRemoveObj(el, e) { e.stopPropagation(); PAT.removeObj(el.dataset.id); },
    patSuppression() { PAT.toggleSuppression(); },
    skipThem(el) { PAT.skipThem(el.dataset.id); },
    clearProfile(el, e) { e.stopPropagation(); S.dialog.profile = null; S.dialog.activeBox = 'profile'; A.render(); },
    dlgRev(el) { const v = el.dataset.v === '1'; S.dialog.rev = v; A.edit((d) => { d.p.rev = v; }, { noUndo: true, preview: true }); },
    cancelDialog() { A.cancelDialog(); }, okDialog() { A.okDialog(); },
    sketchOnFace(el) { const m = VP.model(); const f = m.faces.find((x) => x.id === el.dataset.id); sketchOn({ face: f }); },
    cancelPick() { endPickPlane(); },
    clearSel() { A.setSel([]); },
    finishSketch() { SK.finish(); }, cancelSketch() { SK.cancel(); },
    skOpt(el) { SK.opt(el.dataset.k); },
    skDemoOver() { SK.demoOver(); },
    skModOk() { SK.modifyAccept(); }, skModCancel() { SK.cancelModify(); },
    skMakeDriven() { SK.makeDriven(); },
    skDriven(el) { SK.setDriven(el.dataset.id, !el.classList.contains('on')); },
    skUnjoin(el) { SK.unjoin(el.dataset.p, el.dataset.c); },
    skJoin() { SK.join(); },
    skSelPoint(el) { SK.K.sel = [{ kind: 'point', id: el.dataset.p }]; A.render(); },
    skDelSel() { SK.del(); },
    skClearSel() { SK.K.sel = []; A.render(); },
    skMeasureClose() { SK.closeMeasure(); },
    skMeasureClear() { SK.clearMeasure(); },
    lookAtSketch() { SK.lookAt(); },
    constraint() { A.toast('Select the curves or points first; the relation applies to them.'); },
  };
  function cmdItem(x) { if (x === 'sep' || x.head) return x; const c = A.CMD[x]; return { label: c.label, icon: c.icon, keys: c.keys, run: () => runCmd(x) }; }
  function selectFeature(id) {
    S.selFeature = S.selFeature === id && !S.dialog ? id : id; S.sel = []; S.selRow = null;
    const d = doc(); if (d && d.outside[id]) { /* seen on select, cleared by the button */ }
    A.syncHighlight(); A.render();
  }
  function editFeature(id) {
    const d = doc(); const f = d && d.features.find((x) => x.id === id); if (!f) return;
    S.modal = null;
    if (f.type === 'sketch') { if (id === 's0000001') SK.enter({ featureId: id, plane: 'XZ', mode: 'edit' }); else if (d.newSketches[id]) SK.enter({ plane: d.newSketches[id].plane in SK.FRAMES ? d.newSketches[id].plane : 'XY', name: f.name }); else A.toast('This prototype edits Bracket profile; ' + f.name + ' would open the same way.'); return; }
    if (PAT.isPattern(f.type)) { PAT.open(f.type, id); return; }
    if (d.kind === 'plate') { A.toast('This prototype edits the plate\'s patterns and mirror; ' + f.name + ' opens its dialog as the bracket\'s do.'); return; }
    if (f.type === 'pad') A.openDialog('pad', id);
    else if (f.type === 'fillet') A.openDialog('fillet', id);
    else if (f.type === 'pocket') A.openDialog('pocket', id);
  }
  document.addEventListener('click', (e) => {
    if (S.menu && !e.target.closest('#menu')) { closeMenu(); if (!e.target.closest('[data-act$="Menu"]')) return; }
    const el = e.target.closest('[data-act]'); if (!el || el.disabled) return;
    const fn = ACT[el.dataset.act]; if (fn) fn(el, e);
  });
  document.addEventListener('dblclick', (e) => {
    const el = e.target.closest('[data-act="tlItem"],[data-act="featureRow"],[data-act="bodyRow"]'); if (!el) return;
    if (el.dataset.act === 'bodyRow') return;
    editFeature(el.dataset.id && el.dataset.act === 'tlItem' ? el.dataset.id : el.dataset.feature);
  });
  document.addEventListener('input', (e) => {
    const k = e.target.dataset.input; if (!k) return;
    if (k === 'dist' || k === 'rad' || k === 'name') A.dialogInput(k, e.target.value);
    else if (k === 'pat') PAT.input(e.target.dataset.k, e.target.value);
    else if (k === 'scq') { S.modal.q = e.target.value; A.render(); }
    else if (k === 'palq') { S.palette.q = e.target.value; S.palette.idx = 0; A.render(); }
    else if (k === 'skmod') SK.modifyInput(e.target.value);
  });

  // Hover links between a candidate and the canvas, and Select Other's list.
  document.addEventListener('mouseover', (e) => {
    const c = e.target.closest('[data-cand]');
    const v = c ? c.dataset.cand : null;
    if (v !== S.candHot) { S.candHot = v; A.syncHighlight(); }
    const tl = e.target.closest('[data-act="tlItem"]');
    const hf = tl ? tl.dataset.id : null;
    if (hf !== S.hoverFeature) { S.hoverFeature = hf; A.syncHighlight(); }
    const mh = e.target.closest('[data-hover]');
    if (S.menu && S.menu.cls === 'select-other') { const i = mh ? +mh.dataset.hover : null; if (i != null && S.menu.kfocus !== i) { S.menu.kfocus = i; soHighlight(); A.renderFloating(); } }
    if (S.sketch && SK.K) {
      const it = S.menu && mh ? S.menu.items[+mh.dataset.hover] : null; SK.menuHover(it && it.hl);
      const r = e.target.closest('[data-mhl]'); SK.measureHot(r ? r.dataset.mhl.replace(/^\d/, '') : null);
    }
    tooltip(e);
  });

  // ───────── Tooltips (egui's on_hover_ui: title, shortcut, one line) ─────────
  let tipTimer = 0, tipEl = null;
  function tooltip(e) {
    const el = e.target.closest('[data-tip]');
    if (el === tipEl) return;
    tipEl = el; clearTimeout(tipTimer); hideTip();
    if (!el || !el.dataset.tip) return;
    tipTimer = setTimeout(() => {
      if (!document.body.contains(el)) return;
      const r = el.getBoundingClientRect(); const t = document.createElement('div'); t.className = 'tooltip'; t.id = 'tip';
      const keys = (el.dataset.tipk || '').split(' ').filter(Boolean);
      t.innerHTML = `<div class="tt-title">${A.esc(el.dataset.tip)}${keys.length ? `<span style="margin-left:auto;display:flex;gap:2px">${A.kbd(keys)}</span>` : ''}</div>${el.dataset.tipd ? `<div class="tt-desc">${el.dataset.tipd}</div>` : ''}`;
      document.body.appendChild(t);
      const tr = t.getBoundingClientRect();
      let x = Math.min(innerWidth - tr.width - 8, Math.max(8, r.left + r.width / 2 - tr.width / 2)), y = r.bottom + 6;
      if (y + tr.height > innerHeight - 8) y = r.top - tr.height - 6;
      t.style.left = x + 'px'; t.style.top = y + 'px';
    }, 450);
  }
  function hideTip() { const t = document.getElementById('tip'); if (t) t.remove(); }
  document.addEventListener('mousedown', () => { clearTimeout(tipTimer); hideTip(); });

  // ───────── Menus ─────────
  function menu(x, y, items, extra) { S.menu = Object.assign({ x, y, items, kfocus: null }, extra || {}); A.renderFloating(); }
  function closeMenu() { if (S.menu && S.menu.cls === 'select-other') { S.hover = null; A.syncHighlight(); } if (S.sketch && SK.K) SK.menuHover(null); S.menu = null; A.renderFloating(); }
  function displayMenu(x, y, above) {
    const items = [{ head: 'Visual style' },
      { label: '✓ Shaded with visible edges', run: () => {} },
      { label: 'Shaded', disabled: true, sub: 'later' },
      { label: 'Wireframe', disabled: true, sub: 'later' },
      { label: 'Hidden edges', disabled: true, sub: 'not in M5' },
      'sep', { head: 'Camera' },
      { label: (VP.cam.persp ? '   ' : '✓ ') + 'Orthographic', icon: 'ortho', run: () => { VP.cam.persp = false; VP.request(); A.render(); } },
      { label: (VP.cam.persp ? '✓ ' : '   ') + 'Perspective', icon: 'persp', run: () => { VP.cam.persp = true; VP.request(); A.render(); } },
      'sep', { label: (S.grid !== false ? '✓ ' : '   ') + 'Ground grid', icon: 'grid', run: () => ACT.toggleGrid() }];
    menu(x, y, items); if (above) { requestAnimationFrame(() => { const m = $('#menu'); if (m) m.style.top = (y - m.getBoundingClientRect().height) + 'px'; }); }
  }
  function cubeMenu(x, y) {
    menu(x, y, [{ label: 'Home view', icon: 'home', keys: ['Home'], run: () => VP.home() }, 'sep', { head: 'Views' },
      ...[['Front', 'vFront'], ['Back', 'vBack'], ['Left', 'vLeft'], ['Right', 'vRight'], ['Top', 'vTop'], ['Bottom', 'vBottom'], ['Isometric', 'vIso']].map(([l, c]) => ({ label: l, keys: A.CMD[c].keys, run: () => runCmd(c) })),
      'sep', { head: 'Camera' },
      { label: (VP.cam.persp ? '   ' : '✓ ') + 'Orthographic', run: () => { VP.cam.persp = false; VP.request(); A.render(); } },
      { label: (VP.cam.persp ? '✓ ' : '   ') + 'Perspective', run: () => { VP.cam.persp = true; VP.request(); A.render(); } }]);
  }
  function soHighlight() { const it = S.menu.items[S.menu.kfocus]; if (it && it.target) { S.hover = it.target; A.syncHighlight(); } }
  function openSelectOther(e) {
    const all = VP.pickAll(e.clientX, e.clientY);
    const d = doc(); const m = VP.model();
    const items = [{ head: 'Under the pointer, nearest first' }];
    all.forEach((h) => {
      const ref = h.ref; const kind = h.kind === 'face' ? (ref.kind === 'plane' ? 'Planar face' : 'Cylindrical face') : h.kind === 'edge' ? ref.kind[0].toUpperCase() + ref.kind.slice(1) + ' edge' : 'Vertex';
      items.push({ label: h.id, sub: kind + ' · ' + A.featName(d, ref.maker) + (h.visible === false ? ' · hidden' : ''), icon: { face: 'selFace', edge: 'selEdge', vertex: 'selVertex' }[h.kind], target: { kind: h.kind, id: h.id }, hover: true, run: () => A.setSel([{ kind: h.kind, id: h.id }]) });
    });
    if (items.length === 1) { A.toast('Nothing under the pointer'); return; }
    void m;
    menu(e.clientX + 28, e.clientY - 18, items, { cls: 'select-other', kfocus: 1, foot: `${A.kbd(['Tab'])} or the wheel steps · ${A.kbd(['Enter'])} selects · ${A.kbd(['Esc'])} closes` });
    soHighlight();
  }

  // ───────── Viewport hooks ─────────
  VP.hooks.onHover = function (e) {
    if (!e) { if (S.hover) { S.hover = null; A.syncHighlight(); A.renderStatus(); } return; }
    if (S.sketch) { SK.onMove(e); return; }
    if (S.menu && S.menu.cls === 'select-other') return;
    if (S.pickPlane) {
      const t = VP.pickPlane(e.clientX, e.clientY);
      const hp = t && t.plane ? t.plane : null; const hf = t && t.face ? { kind: 'face', id: t.face.id } : null;
      VP.setPlanes({ XY: true, XZ: true, YZ: true }, hp);
      S.hover = hf; A.syncHighlight(); return;
    }
    if (dragArrow) return;
    const h = VP.pick(e.clientX, e.clientY);
    const nh = h ? { kind: h.kind, id: h.id } : null;
    if (JSON.stringify(nh) !== JSON.stringify(S.hover)) {
      S.hover = nh; A.syncHighlight(); A.renderStatus();
      document.querySelectorAll('.tl-item.hovered-from-canvas').forEach((x) => x.classList.remove('hovered-from-canvas'));
      if (h) { const ref = h.ref; const t = document.querySelector(`.tl-item[data-id="${ref.maker}"]`); t && t.classList.add('hovered-from-canvas'); }
    }
    $('#canvas-wrap').style.cursor = arrowHit(e) ? 'grab' : PAT.overHit(e) ? 'pointer' : '';
  };
  VP.hooks.onLeave = () => { if (S.sketch) { SK.K && (SK.K.cursor = null); A.renderStatus(); VP.request(); } };
  VP.hooks.onClick = function (e) {
    if (S.menu) { closeMenu(); return; }
    if (S.sketch) { SK.onClick(e); return; }
    if (S.pickPlane) { const t = VP.pickPlane(e.clientX, e.clientY); if (t) sketchOn(t); return; }
    const h = VP.pick(e.clientX, e.clientY);
    if (S.dialog && S.dialog.pattern) { if (h) PAT.pick({ kind: h.kind, id: h.id, ref: h.ref, feature: h.kind === 'face' ? h.ref.maker : null }); return; }
    if (S.dialog) { if (h && S.dialog.type === 'fillet' && h.kind === 'edge') A.toast('Prototype: the edge would join the Edges box.'); return; }
    if (!h) { if (S.sel.length || S.selFeature || S.selRow) A.setSel([]); return; }
    const it = { kind: h.kind, id: h.id };
    if (e.ctrlKey || e.metaKey) A.toggleSel(it); else A.setSel([it]);
  };
  VP.hooks.onDblClick = function (e) {
    if (S.sketch) { SK.onDbl(e); return; }
    const h = VP.pick(e.clientX, e.clientY); if (h) selectFeature(h.ref.maker);
  };
  VP.hooks.onBox = function (list, o) {
    if (S.sketch || S.pickPlane) return;
    if (!S.filter.edge) { A.toast('The edge filter is off, and a box selects edges in a part.'); return; }
    if (o.add) list.forEach((x) => { if (!S.sel.some((s) => s.kind === x.kind && s.id === x.id)) S.sel.push(x); });
    else if (o.invert) list.forEach((x) => { const i = S.sel.findIndex((s) => s.kind === x.kind && s.id === x.id); if (i >= 0) S.sel.splice(i, 1); else S.sel.push(x); });
    else S.sel = list;
    S.selFeature = null; A.syncHighlight(); A.render();
    A.toast(`${o.crossing ? 'Crossing' : 'Window'} selection: ${list.length} edge${list.length === 1 ? '' : 's'}`);
  };
  VP.hooks.onContext = function (e) {
    if (S.sketch) { menu(e.clientX, e.clientY, SK.contextItems(e)); return; }
    const d = doc(); if (!d) return;
    const h = VP.pick(e.clientX, e.clientY); const all = VP.pickAll(e.clientX, e.clientY);
    if (h) {
      const ref = h.ref; const maker = A.featName(d, ref.maker);
      menu(e.clientX, e.clientY, [{ head: h.id + ' · ' + (h.kind === 'face' ? (ref.kind === 'plane' ? 'Planar face' : 'Cylindrical face') : h.kind) },
        { label: 'Select Other…', icon: 'target', sub: all.length + ' under the pointer', run: () => openSelectOther(e) },
        { label: 'Edit ' + maker, icon: 'edit', run: () => editFeature(ref.maker) },
        h.kind === 'face' && ref.kind === 'plane' ? { label: 'Sketch on this face', icon: 'sketch', run: () => sketchOn({ face: ref }) } : null,
        h.kind === 'face' && ref.kind === 'plane' ? { label: 'Look At', icon: 'lookAt', keys: ['Mod', '8'], run: () => { A.setSel([{ kind: 'face', id: h.id }]); lookAt(); } } : null,
        h.kind === 'edge' ? { label: 'Fillet', icon: 'fillet', run: () => runCmd('fillet') } : null,
        'sep',
        { label: 'Copy name', icon: 'copy', sub: 'as agents write it', run: () => ACT.copy({ dataset: { text: ref.stable || h.id } }) },
        { label: 'Hide Body1', icon: 'eyeOff', keys: ['H'], run: () => runCmd('hide') },
      ].filter(Boolean));
    } else {
      menu(e.clientX, e.clientY, [{ label: 'Fit', icon: 'fit', keys: ['F6'], run: () => VP.fit() }, { label: 'Home view', icon: 'home', keys: ['Home'], run: () => VP.home() },
        { label: 'Select Other…', icon: 'target', disabled: !all.length, run: () => openSelectOther(e) },
        'sep', { label: 'Clear selection', keys: ['Esc'], disabled: !S.sel.length, run: () => A.setSel([]) },
        { label: 'Create Sketch', icon: 'sketch', run: () => runCmd('createSketch') }]);
    }
  };
  VP.hooks.onCubeMenu = (e) => cubeMenu(e.clientX - 160, e.clientY);
  VP.hooks.onWheel = function (e) {
    if (S.menu && S.menu.cls === 'select-other') { stepSO(e.deltaY > 0 ? 1 : -1); return true; }
    return false;
  };
  VP.hooks.onView = function () { const l = document.querySelector('.view-label'); if (l) { const vn = VP.viewName(); l.textContent = (VP.cam.persp ? 'Perspective' : 'Orthographic') + (vn ? ' · ' + vn[0].toUpperCase() + vn.slice(1) : ''); } if (S.sketch && SK.K && SK.K.modify) A.renderCanvasUI(); };
  function stepSO(dir) {
    const items = S.menu.items; let i = S.menu.kfocus ?? 0;
    for (let n = 0; n < items.length; n++) { i = (i + dir + items.length) % items.length; if (items[i] && items[i].target) break; }
    S.menu.kfocus = i; soHighlight(); A.renderFloating();
  }

  // Pad's distance arrow, drawn over the viewport and dragged along the pad's direction.
  let dragArrow = null;
  function arrowGeom() {
    const d = doc(); if (!S.dialog || S.dialog.type !== 'pad' || !S.dialog.profile || !d) return null;
    const base = new THREE.Vector3(18, d.p.D, 14); const dir = new THREE.Vector3(0, d.p.rev ? 1 : -1, 0);
    const a = VP.project(base), b = VP.project(base.clone().addScaledVector(dir, 10));
    const ppu = Math.hypot(b.x - a.x, b.y - a.y) / 10; if (ppu < 0.6) return { a, flat: true };
    const ux = (b.x - a.x) / (ppu * 10), uy = (b.y - a.y) / (ppu * 10);
    return { a, ux, uy, ppu, tip: { x: a.x + ux * 56, y: a.y + uy * 56 } };
  }
  function arrowHit(e) {
    const g = arrowGeom(); if (!g || g.flat) return false;
    const r = $('#gl').getBoundingClientRect(); const m = { x: e.clientX - r.left, y: e.clientY - r.top };
    return Math.hypot(m.x - g.tip.x, m.y - g.tip.y) < 14 || segD(m, g.a, g.tip) < 6;
  }
  function segD(p, a, b) { const dx = b.x - a.x, dy = b.y - a.y, L = dx * dx + dy * dy; let t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / L; t = Math.max(0, Math.min(1, t)); return Math.hypot(p.x - a.x - t * dx, p.y - a.y - t * dy); }
  VP.hooks.onPointerDown = function (e) {
    if (S.sketch && SK.pointerDown && SK.pointerDown(e)) return true;
    if (PAT.pointerDown(e)) return true;
    if (arrowHit(e)) { const g = arrowGeom(); dragArrow = { x: e.clientX, y: e.clientY, D0: doc().p.D, g }; return true; }
    return false;
  };
  VP.hooks.onPointerDrag = function (e) {
    if (!dragArrow) return; const g = dragArrow.g;
    const dd = ((e.clientX - dragArrow.x) * g.ux + (e.clientY - dragArrow.y) * g.uy) / g.ppu;
    const v = Math.max(0.5, Math.round((dragArrow.D0 + dd) * 2) / 2);
    if (v !== doc().p.D) { S.dialog.text = String(v); A.dialogInput('dist', String(v)); }
  };
  VP.hooks.onPointerUp = function () { dragArrow = null; };
  VP.hooks.overlay = function (w, h) {
    if (S.sketch) return SK.overlay(w, h);
    let s = '';
    const m = VP.model();
    if (m) {
      const dot = (id, col, r) => { const v = m.verts.find((x) => x.id === id); if (!v) return ''; const p = VP.project(v.p); return `<circle cx="${p.x}" cy="${p.y}" r="${r}" fill="${col}" stroke="#000" stroke-width="1"/>`; };
      if (S.hover && S.hover.kind === 'vertex') s += dot(S.hover.id, '#ffffff', 4.5);
      S.sel.filter((x) => x.kind === 'vertex').forEach((x) => { s += dot(x.id, '#4676ff', 4.5); });
    }
    const g = arrowGeom();
    if (g && !g.flat) {
      const col = dragArrow ? '#ffffff' : '#4676ff';
      s += `<line x1="${g.a.x}" y1="${g.a.y}" x2="${g.tip.x}" y2="${g.tip.y}" stroke="${col}" stroke-width="2.5"/>`;
      const px = -g.uy, py = g.ux;
      s += `<path d="M${g.tip.x + g.ux * 12},${g.tip.y + g.uy * 12} L${g.tip.x + px * 6},${g.tip.y + py * 6} L${g.tip.x - px * 6},${g.tip.y - py * 6} Z" fill="${col}"/>`;
      s += `<circle cx="${g.a.x}" cy="${g.a.y}" r="3.5" fill="${col}"/>`;
      const lx = g.tip.x + g.ux * 22 + 6, ly = g.tip.y + g.uy * 22;
      const lbl = A.fmt(doc().p.D) + ' mm';
      s += `<rect x="${lx}" y="${ly - 10}" width="${lbl.length * 6.6 + 12}" height="20" rx="4" fill="#212121" stroke="${col}"/><text x="${lx + 6}" y="${ly + 4}" fill="#fff" font-size="11.5" font-family="Inter Silico, Inter">${lbl}</text>`;
    }
    s += PAT.overlay();
    return s;
  };

  // ───────── Keyboard ─────────
  function isMod(e) { return A.MAC ? e.metaKey : e.ctrlKey; }
  document.addEventListener('keydown', (e) => {
    const t = e.target; const typing = t.tagName === 'INPUT' || t.tagName === 'TEXTAREA';
    const key = e.key;
    // Menus and Select Other
    if (S.menu) {
      const items = S.menu.items; const sel = (i) => items[i] && items[i] !== 'sep' && !items[i].head && !items[i].disabled;
      if (key === 'Escape') { e.preventDefault(); closeMenu(); return; }
      if (key === 'ArrowDown' || (key === 'Tab' && !e.shiftKey)) { e.preventDefault(); let i = S.menu.kfocus ?? -1; for (let n = 0; n < items.length; n++) { i = (i + 1) % items.length; if (sel(i)) break; } S.menu.kfocus = i; if (S.menu.cls === 'select-other') soHighlight(); A.renderFloating(); return; }
      if (key === 'ArrowUp' || (key === 'Tab' && e.shiftKey)) { e.preventDefault(); let i = S.menu.kfocus ?? items.length; for (let n = 0; n < items.length; n++) { i = (i - 1 + items.length) % items.length; if (sel(i)) break; } S.menu.kfocus = i; if (S.menu.cls === 'select-other') soHighlight(); A.renderFloating(); return; }
      if (key === 'Enter' && S.menu.kfocus != null) { e.preventDefault(); const it = items[S.menu.kfocus]; closeMenu(); it.run && it.run(); return; }
    }
    if (S.palette) {
      const items = A.paletteItems();
      if (key === 'Escape') { e.preventDefault(); S.palette = null; A.render(); return; }
      if (key === 'ArrowDown') { e.preventDefault(); S.palette.idx = Math.min(items.length - 1, (S.palette.idx || 0) + 1); A.render(); return; }
      if (key === 'ArrowUp') { e.preventDefault(); S.palette.idx = Math.max(0, (S.palette.idx || 0) - 1); A.render(); return; }
      if (key === 'Enter') { e.preventDefault(); const it = items[S.palette.idx || 0]; S.palette = null; A.render(); if (it) runCmd(it[0]); return; }
      return;
    }
    if (S.modal) {
      if (key === 'Escape') { e.preventDefault(); if (S.modal.type === 'overdefine') SK.overdefineChoice('cancel'); else { S.modal = null; A.syncHighlight(); A.render(); } return; }
      if (S.modal.type === 'overdefine' && (key === '1' || key === '2')) { SK.overdefineChoice(key === '1' ? 'driven' : 'driving'); return; }
      if (key === 'Enter') {
        if (S.modal.type === 'saveAs') { e.preventDefault(); ACT.doSaveAs(); }
        else if (S.modal.type === 'closeDirty') { e.preventDefault(); ACT.closeSave({ dataset: { id: S.modal.uid } }); }
      }
      return;
    }
    // The Modify box
    if (t.dataset && t.dataset.key === 'sk-mod') {
      if (key === 'Enter') { e.preventDefault(); SK.modifyAccept(); return; }
      if (key === 'Escape') { e.preventDefault(); SK.cancelModify(); return; }
      return;
    }
    if (typing) {
      if (key === 'Enter' && S.dialog) { e.preventDefault(); t.blur(); A.okDialog(); return; }
      if (key === 'Escape') { e.preventDefault(); t.blur(); if (S.dialog) { if (!escChain()) {} } return; }
      return;
    }
    if (key === 'Escape') { e.preventDefault(); escChain(); return; }
    if (key === 'Enter' && S.dialog) { e.preventDefault(); A.okDialog(); return; }
    if (key === '?' || (key === '/' && e.shiftKey)) { e.preventDefault(); runCmd('shortcuts'); return; }
    if (isMod(e)) {
      const k = key.toLowerCase();
      const map = { n: 'newDoc', o: 'open', s: e.shiftKey ? 'saveAs' : 'save', w: 'close', z: e.shiftKey ? 'redo' : 'undo', y: 'redo', b: e.shiftKey ? 'rebuildAll' : 'rebuild', r: 'reload', k: 'search', '1': 'vFront', '2': 'vBack', '3': 'vLeft', '4': 'vRight', '5': 'vTop', '6': 'vBottom', '7': 'vIso', '8': 'lookAt', enter: 'finishSketch' };
      if (map[k]) { if (map[k] === 'finishSketch' && !S.sketch) return; e.preventDefault(); runCmd(map[k]); }
      return;
    }
    if (e.altKey) return;
    if (key === 'F6' || (key === 'f' && !S.sketch)) { e.preventDefault(); runCmd('fit'); return; }
    if (key === 'Home') { e.preventDefault(); runCmd('home'); return; }
    if (key === 'F2') { e.preventDefault(); runCmd('rename'); return; }
    if (key === 'F5') { e.preventDefault(); A.toast('F5 shows the selection filters. They live in the toolbar, under SELECT.'); return; }
    if (key === 's' || key === 'S') { e.preventDefault(); runCmd('search'); return; }
    if (S.sketch) {
      if ((key === 'Delete' || key === 'Backspace') && SK.K && SK.K.sel && SK.K.sel.length) { e.preventDefault(); SK.del(); return; }
      const m = { l: 'sLine', r: 'sRect', c: 'sCircle', a: 'sArc', p: 'sPoint', t: 'sTrim', d: 'sDim', x: 'sConstruction' }[key.toLowerCase()];
      if (m) { e.preventDefault(); runCmd(m); }
      return;
    }
    if (!doc()) return;
    const m = { x: 'filterFace', e: 'filterEdge', v: 'filterVertex', h: 'hide' }[key.toLowerCase()];
    if (m) { e.preventDefault(); runCmd(m); }
    if (key === 'Delete' || key === 'Backspace') { if (S.selFeature) { e.preventDefault(); runCmd('del'); } }
    if ((key === 'ArrowRight' || key === 'ArrowLeft') && S.selFeature) {
      const d = doc(); const i = d.features.findIndex((f) => f.id === S.selFeature);
      const j = Math.max(0, Math.min(d.features.length - 1, i + (key === 'ArrowRight' ? 1 : -1))); selectFeature(d.features[j].id);
    }
    if (S.view === 'start' && (key === 'ArrowDown' || key === 'ArrowUp')) {
      const rows = [...document.querySelectorAll('.recent-row')]; const i = rows.indexOf(document.activeElement);
      const j = key === 'ArrowDown' ? Math.min(rows.length - 1, i + 1) : Math.max(0, i - 1); rows[j] && rows[j].focus(); e.preventDefault();
    }
  });
  // Esc goes to the nearest thing first (M5.0 section 3).
  function escChain() {
    if (S.navMode) { S.navMode = null; VP.navMode = null; A.render(); return true; }
    if (S.sketch && SK.esc()) return true;
    if (S.dialog && S.rebuild.state === 'running') { A.stopRebuild(); return true; }
    if (S.dialog) { A.cancelDialog(); return true; }
    if (S.pickPlane) { endPickPlane(); return true; }
    if (S.banner && S.banner.kind === 'reloaded') { S.banner = null; A.render(); return true; }
    if (S.sel.length || S.selFeature || S.selRow) { A.setSel([]); return true; }
    if (S.rebuild.state === 'running') { A.stopRebuild(); return true; }
    return false;
  }

  // ───────── Timeline rollback bar (Onshape's), dragged between features ─────────
  document.addEventListener('mousedown', (e) => {
    const r = e.target.closest('[data-roll]'); if (!r) return;
    e.preventDefault(); r.classList.add('dragging');
    const move = (ev) => {
      const items = [...document.querySelectorAll('.tl-item')]; const d = doc(); let idx = items.length;
      for (let i = 0; i < items.length; i++) { const b = items[i].getBoundingClientRect(); if (ev.clientX < b.left + b.width / 2) { idx = i; break; } }
      const nr = idx >= d.features.length ? null : Math.max(1, idx);
      if (nr !== d.rollback) { d.rollback = nr; A.evaluate(d); d.built = d.next; d.health = d.next.health; d.problems = d.next.problems; A.publish(d); A.render(); }
    };
    const up = () => { window.removeEventListener('mousemove', move); window.removeEventListener('mouseup', up); const d = doc(); if (d.rollback != null) A.toast('Rolled back: the features after the bar are not built. Drag it to the end to see them again.'); };
    window.addEventListener('mousemove', move); window.addEventListener('mouseup', up);
  });

  // ───────── The side panes' borders, dragged to resize them (Trevin, 2026-11-09) ─────────
  // The column moves by the drag's distance from the press, the canvas taking up the
  // difference, between the pane's least width and what leaves the canvas its own.
  const MIN = { browser: 200, right: 232, canvas: 340 };
  document.addEventListener('mousedown', (e) => {
    const sash = e.target.closest('[data-sash]'); if (!sash || e.button !== 0) return;
    e.preventDefault();
    const side = sash.dataset.sash; const other = side === 'browser' ? 'right' : 'browser';
    const root = document.documentElement;
    const width = (name) => $(name === 'browser' ? '#browser' : '#right').getBoundingClientRect().width;
    const start = width(side); const held = width(other); const x0 = e.clientX;
    // The other held as shown, but a collapsed browser's strip, which keeps its width for later.
    if (!(other === 'browser' && $('#main').classList.contains('browser-collapsed'))) root.style.setProperty('--' + other + '-w', held + 'px');
    document.body.classList.add('sashing');
    const move = (ev) => {
      const travel = side === 'browser' ? ev.clientX - x0 : x0 - ev.clientX;
      const most = Math.floor($('#main').getBoundingClientRect().width - held - MIN.canvas);
      const w = Math.max(MIN[side], Math.min(most, Math.round(start + travel)));
      root.style.setProperty('--' + side + '-w', w + 'px');
    };
    const up = () => { window.removeEventListener('mousemove', move); window.removeEventListener('mouseup', up); document.body.classList.remove('sashing'); };
    window.addEventListener('mousemove', move); window.addEventListener('mouseup', up);
  });

  // Right-click on the timeline and the browser.
  document.addEventListener('contextmenu', (e) => {
    const t = e.target.closest('[data-act="tlItem"],[data-act="featureRow"]'); if (!t) return;
    e.preventDefault(); const id = t.dataset.act === 'tlItem' ? t.dataset.id : t.dataset.feature; const d = doc(); const f = d.features.find((x) => x.id === id);
    selectFeature(id);
    menu(e.clientX, e.clientY, [{ head: f.name },
      { label: f.type === 'sketch' ? 'Edit Sketch' : 'Edit feature', icon: 'edit', sub: 'double-click', run: () => editFeature(id) },
      { label: 'Rename', keys: ['F2'], run: () => runCmd('rename') },
      { label: d.suppressed[id] ? 'Unsuppress' : 'Suppress', icon: 'suppress', run: () => ACT.suppress({ dataset: { id } }) },
      t.dataset.act === 'tlItem' ? { label: 'Find in browser', run: () => { S.expanded.sketches = true; A.render(); } } : { label: 'Find in timeline', run: () => {} },
      { label: 'Roll back to here', run: () => { d.rollback = d.features.indexOf(f) + 1 >= d.features.length ? null : d.features.indexOf(f) + 1; A.evaluate(d); d.built = d.next; d.health = d.next.health; d.problems = d.next.problems; A.publish(d); A.render(); } },
      'sep', { label: 'Delete', danger: true, keys: ['Del'], run: () => runCmd('del') }]);
  });

  // ───────── Prototype states ─────────
  function proto(k) {
    S.proto.current = k; S.modal = null; S.menu = null; S.palette = null; S.slow = false;
    if (S.sketch) SK.cancel();
    S.navMode = null; VP.navMode = null;
    $('#canvas-wrap').classList.remove('tool-line');
    switch (k) {
      case '1': S.view = 'start'; S.noDocVariant = 'start'; if (!S.docs.length) S.active = null; A.render(); break;
      case '1b': S.docs = []; S.active = null; S.view = 'start'; S.noDocVariant = 'canvas'; VP.setModel(null); A.render(); VP.fitBox(VP.modelBox(), VP.VIEWS.iso); break;
      case '2': openBracket(true); break;
      case '2r': openBracket(true); S.slow = true; setTimeout(() => A.startRebuild(doc()), 350); break;
      case '3': openBracket(true); setTimeout(() => A.openDialog('pad', 'p0000001'), 300); break;
      case '3e': openBracket(true); setTimeout(() => { A.openDialog('pad', 'p0000001'); S.dialog.text = '8'; A.dialogInput('dist', '8'); }, 300); break;
      case '4': openBracket(true); setTimeout(() => {
        const m = VP.model(); const corner = m.edges.find((e) => e.lengthwise && e.maker === 'f0000001');
        A.setSel([{ kind: 'face', id: 'Face5' }, { kind: 'edge', id: corner.id }]);
        setTimeout(() => {
          const p = VP.project(new THREE.Vector3(42.75, 12, 5)); const r = $('#gl').getBoundingClientRect();
          openSelectOther({ clientX: r.left + p.x, clientY: r.top + p.y });
        }, 320);
      }, 350); break;
      case '5': openBracket(true); setTimeout(() => SK.enter({ featureId: 's0000001', plane: 'XZ', mode: 'placing' }), 300); break;
      case '5n': S.docs = S.docs.filter((d) => d.kind !== 'empty'); newDoc('Untitled-1'); setTimeout(() => { SK.enter({ plane: 'XY', name: 'Sketch1' }); SK.seedNew(); }, 350); break;
      case '6a': openBracket(true); setTimeout(() => { agentEdit('filletFail'); setTimeout(() => selectFeature('f0000001'), 700); }, 300); break;
      case '6b': openBracket(true); setTimeout(() => { agentEdit('brokenRef'); setTimeout(() => selectFeature('f0000001'), 700); }, 300); break;
      case '6c': openBracket(true); setTimeout(() => agentEdit('radius'), 300); break;
      case '6d': openBracket(true); setTimeout(() => { A.edit((d) => { d.p.D = 45; }, { label: 'Legs: 40 mm → 45 mm' }); A.edit((d) => { d.p.r = 4; }, { label: 'Inside corner: 5 mm → 4 mm' }); setTimeout(() => agentEdit('radius'), 600); }, 300); break;
      case '7': openBracket(true); setTimeout(() => runCmd('shortcuts'), 300); break;
      default: if (!PAT.protoState(k)) SK.protoState(k, openBracket);
    }
    A.render();
  }
  function createMenu(r) {
    menu(r.left, r.bottom + 2, ['createSketch', 'sep', 'pad', 'pocket', 'revolve', 'groove', 'sep', { head: 'Pattern' }, 'rectPattern', 'circPattern', 'sep', 'mirror'].map(cmdItem));
  }
  PAT.setMenu((x, y, items) => menu(x, y, items));
  window.PROTO = { proto, agentEdit, activate, createMenu, menu, editFeature, runCmd };

  // ───────── Start ─────────
  VP.init($('#canvas-wrap'), $('#gl'), $('#overlay'));
  VP.filter = S.filter;
  A.render();
  setTimeout(() => { S.workers = 'ready'; A.renderStatus(); }, 1400);
  const startK = (location.hash || '').slice(1);
  if (startK && /^[0-9a-z]+$/.test(startK)) setTimeout(() => proto(startK), 50);
})();
