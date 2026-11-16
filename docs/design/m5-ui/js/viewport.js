/* The viewport. Stands in for silico-render on re_renderer: faces lit by
   re_renderer's simple_lighting (ambient 0.2 and two fixed lights), edges as
   screen-space strips of a fixed width in points, per-face picking, an
   orthographic camera by default (open item 27), Fusion's mouse preset, and
   redraws only when something changed (as the app's waker asks egui). */
(function () {
  const T = THREE;
  const DEG = Math.PI / 180;
  const BODY = new T.Color('#b9c0ca');
  const EDGE = new T.Color('#101010');
  const HOVER = new T.Color('#ffffff');
  const SELECT = new T.Color('#2f7bff');
  const SELECT_EDGE = new T.Color('#5b95ff');
  const CAND = new T.Color('#ff7a0c');
  const MAKER = new T.Color('#d4dbff');

  const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  const faceVS = `
    varying vec3 vN; varying vec3 vP;
    void main(){ vN = normalize(normalMatrix*normal); vec4 mv = modelViewMatrix*vec4(position,1.0); vP = mv.xyz; gl_Position = projectionMatrix*mv; }`;
  const faceFS = `
    uniform vec3 uColor; uniform vec3 uTint; uniform float uTintAmt; uniform float uDim; uniform float uOpacity;
    varying vec3 vN; varying vec3 vP;
    void main(){
      vec3 n = normalize(vN);
      vec3 toEye = isOrthographic ? vec3(0.,0.,1.) : normalize(-vP);
      if (dot(n, toEye) < 0.0) n = -n;
      float s = 0.2 + clamp(dot(normalize(vec3(1.,2.,3.)), n),0.,1.) + 0.5*clamp(dot(normalize(vec3(-1.,-3.,-5.)), n),0.,1.);
      s = clamp(s, 0., 1.);
      vec3 c = uColor*s;
      c = mix(c, uTint*(0.5+0.5*s), uTintAmt);
      c = mix(vec3(0.0), c, uDim);
      gl_FragColor = vec4(c, uOpacity);
    }`;
  function faceMaterial(color) {
    return new T.ShaderMaterial({
      vertexShader: faceVS, fragmentShader: faceFS, side: T.DoubleSide,
      uniforms: { uColor: { value: (color || BODY).clone() }, uTint: { value: new T.Color() }, uTintAmt: { value: 0 }, uDim: { value: 1 }, uOpacity: { value: 1 } },
      polygonOffset: true, polygonOffsetFactor: 1, polygonOffsetUnits: 1,
    });
  }

  // Edges: one quad per segment, widened in screen space (re_renderer's lines take a
  // width in UI points when negative; this is the same idea).
  const lineVS = `
    uniform vec2 uRes; uniform float uWidth;
    attribute vec3 aA; attribute vec3 aB; attribute vec2 aC;
    void main(){
      vec4 ca = projectionMatrix*modelViewMatrix*vec4(aA,1.0);
      vec4 cb = projectionMatrix*modelViewMatrix*vec4(aB,1.0);
      vec2 sa = ca.xy/ca.w*uRes*0.5; vec2 sb = cb.xy/cb.w*uRes*0.5;
      vec2 d = sb - sa; float L = length(d); d = L > 1e-5 ? d/L : vec2(1.,0.);
      vec2 nrm = vec2(-d.y, d.x);
      vec4 c = mix(ca, cb, aC.x);
      vec2 off = nrm*aC.y*uWidth*0.5 + d*(aC.x*2.0-1.0)*uWidth*0.5;
      c.xy += off/(uRes*0.5)*c.w;
      c.z -= 0.0004*c.w;
      gl_Position = c;
    }`;
  const lineFS = `uniform vec3 uColor; uniform float uOpacity; void main(){ gl_FragColor = vec4(uColor, uOpacity); }`;
  function lineMesh(pts, color, width, opacity, closed) {
    const A = [], B = [], C = [], idx = [];
    let k = 0;
    const segs = [];
    for (let i = 0; i < pts.length - 1; i++) segs.push([pts[i], pts[i + 1]]);
    if (closed) segs.push([pts[pts.length - 1], pts[0]]);
    for (const [a, b] of segs) {
      for (const c of [[0, -1], [0, 1], [1, -1], [1, 1]]) { A.push(a.x, a.y, a.z); B.push(b.x, b.y, b.z); C.push(c[0], c[1]); }
      idx.push(k, k + 2, k + 1, k + 1, k + 2, k + 3); k += 4;
    }
    const g = new T.BufferGeometry();
    g.setAttribute('position', new T.Float32BufferAttribute(A, 3));
    g.setAttribute('aA', new T.Float32BufferAttribute(A, 3));
    g.setAttribute('aB', new T.Float32BufferAttribute(B, 3));
    g.setAttribute('aC', new T.Float32BufferAttribute(C, 2));
    g.setIndex(idx);
    const m = new T.ShaderMaterial({
      vertexShader: lineVS, fragmentShader: lineFS, transparent: (opacity ?? 1) < 1,
      uniforms: { uRes: { value: new T.Vector2(1, 1) }, uWidth: { value: width }, uColor: { value: new T.Color(color) }, uOpacity: { value: opacity ?? 1 } },
    });
    const mesh = new T.Mesh(g, m); mesh.frustumCulled = false;
    return mesh;
  }

  const VP = {
    cam: { az: 35 * DEG, el: 24 * DEG, target: new T.Vector3(30, 20, 18), height: 120, persp: false },
    filter: { face: true, edge: true, vertex: true },
    navMode: null, // 'orbit' | 'pan' from the navigation bar
    hooks: {}, // onHover, onClick, onContext, onBox, onPlanePick, overlay, onPointer, onDblClick
    mode: 'select',
    hl: { hover: null, sel: [], cand: [], maker: null, previewFaces: null },
    dim: 1,
  };
  let renderer, scene, camera, ocam, pcam, wrap, canvas, overlay;
  let modelGroup, gridGroup, planeGroup, sketchGroup;
  let faceMeshes = [], edgeMeshes = [], model = null;
  let cubeScene, cubeCam, cube, cubePatch, cubeHover = null;
  let needs = true, anim = null;
  const dpr = () => Math.min(window.devicePixelRatio || 1, 2);
  const CUBE = 92, CUBE_M = 16;

  VP.init = function (wrapEl, canvasEl, overlayEl) {
    wrap = wrapEl; canvas = canvasEl; overlay = overlayEl;
    // Transparent: the canvas's gradient (styles.css's #canvas-wrap, Trevin's change of
    // 2026-11-09) shows behind the part, as egui's painter under re_renderer's frame.
    renderer = new T.WebGLRenderer({ canvas, antialias: true, alpha: true });
    renderer.setClearColor(0x000000, 0);
    renderer.autoClear = false;
    scene = new T.Scene();
    ocam = new T.OrthographicCamera(-1, 1, 1, -1, 1, 8000);
    pcam = new T.PerspectiveCamera(30, 1, 1, 8000);
    modelGroup = new T.Group(); gridGroup = new T.Group(); planeGroup = new T.Group(); sketchGroup = new T.Group();
    scene.add(gridGroup, modelGroup, planeGroup, sketchGroup);
    buildGrid();
    buildPlanes();
    buildCube();
    new ResizeObserver(() => { resize(); }).observe(wrap);
    resize();
    bindInput();
    loop();
    // Fallback when the page gets no animation frames (background tabs, headless capture).
    setInterval(() => { if (anim) stepAnim(); if (needs || anim) { needs = false; render(); } }, 150);
  };

  function resize() {
    const w = wrap.clientWidth, h = wrap.clientHeight;
    renderer.setPixelRatio(dpr());
    renderer.setSize(w, h, false);
    if (w > 0 && h > 0) render(); // a resize clears the canvas: draw now, not next frame
  }
  VP.request = () => { needs = true; };
  VP.debug = () => ({ lost: renderer.getContext().isContextLost(), cam: { az: VP.cam.az, el: VP.cam.el, h: VP.cam.height, t: VP.cam.target.toArray() }, anim: !!anim });
  VP.size = () => ({ w: wrap.clientWidth, h: wrap.clientHeight });

  function dirOf(az, el) { return new T.Vector3(Math.cos(el) * Math.sin(az), -Math.cos(el) * Math.cos(az), Math.sin(el)); }
  function upOf(az, el) { return new T.Vector3(-Math.sin(el) * Math.sin(az), Math.sin(el) * Math.cos(az), Math.cos(el)); }
  function updateCamera() {
    const { w, h } = VP.size(); const aspect = w / Math.max(h, 1);
    const c = VP.cam; const d = dirOf(c.az, c.el); const up = upOf(c.az, c.el);
    if (c.persp) {
      camera = pcam; pcam.aspect = aspect; pcam.fov = 30;
      const dist = (c.height / 2) / Math.tan(15 * DEG);
      pcam.position.copy(c.target).addScaledVector(d, dist); pcam.near = Math.max(dist / 100, 0.1); pcam.far = dist * 50;
      pcam.up.copy(up); pcam.lookAt(c.target); pcam.updateProjectionMatrix();
    } else {
      camera = ocam; const hh = c.height / 2;
      ocam.left = -hh * aspect; ocam.right = hh * aspect; ocam.top = hh; ocam.bottom = -hh;
      ocam.position.copy(c.target).addScaledVector(d, 2000); ocam.up.copy(up); ocam.lookAt(c.target); ocam.updateProjectionMatrix();
    }
    camera.updateMatrixWorld();
  }

  function loop() {
    requestAnimationFrame(loop);
    if (anim) { stepAnim(); needs = true; }
    if (!needs) return;
    needs = false;
    render();
  }
  function render() {
    updateCamera();
    const { w, h } = VP.size(); const r = dpr();
    const res = new T.Vector2(w * r, h * r);
    scene.traverse((o) => { if (o.material && o.material.uniforms && o.material.uniforms.uRes) o.material.uniforms.uRes.value.copy(res); });
    renderer.setScissorTest(false); renderer.setViewport(0, 0, w, h); renderer.clear();
    renderer.render(scene, camera);
    // View cube, top right
    if (VP.showCube !== false) {
      const x = w - CUBE - CUBE_M, y = h - CUBE - CUBE_M;
      renderer.setScissorTest(true); renderer.setScissor(x, y, CUBE, CUBE); renderer.setViewport(x, y, CUBE, CUBE);
      renderer.clearDepth();
      const d = dirOf(VP.cam.az, VP.cam.el);
      cubeCam.position.copy(d).multiplyScalar(10); cubeCam.up.copy(upOf(VP.cam.az, VP.cam.el)); cubeCam.lookAt(0, 0, 0); cubeCam.updateMatrixWorld();
      renderer.render(cubeScene, cubeCam);
      renderer.setScissorTest(false);
    }
    drawOverlay();
  }

  // ── Grid, axes, origin planes ──
  function buildGrid() {
    const pos = [], col = [];
    const R = 200, push = (a, b, c, alpha) => { pos.push(...a, ...b); for (let i = 0; i < 2; i++) col.push(c.r, c.g, c.b, alpha); };
    const minor = new T.Color('#3a3939'), major = new T.Color('#646363');
    for (let v = -R; v <= R; v += 10) {
      if (v === 0) continue;
      const c = v % 50 === 0 ? major : minor; const a = v % 50 === 0 ? 0.9 : 0.75;
      for (let s = -R; s < R; s += 20) {
        const fa = Math.max(0, 1 - Math.hypot(v, s) / R), fb = Math.max(0, 1 - Math.hypot(v, s + 20) / R);
        pos.push(v, s, -0.05, v, s + 20, -0.05); col.push(c.r, c.g, c.b, a * fa, c.r, c.g, c.b, a * fb);
        pos.push(s, v, -0.05, s + 20, v, -0.05); col.push(c.r, c.g, c.b, a * fa, c.r, c.g, c.b, a * fb);
      }
    }
    const g = new T.BufferGeometry();
    g.setAttribute('position', new T.Float32BufferAttribute(pos, 3));
    g.setAttribute('color', new T.Float32BufferAttribute(col, 4));
    const grid = new T.LineSegments(g, new T.LineBasicMaterial({ vertexColors: true, transparent: true, depthWrite: false }));
    gridGroup.add(grid);
    const axis = (a, b, c) => { const m = lineMesh([a, b], c, 1.5, 0.9); m.renderOrder = 1; gridGroup.add(m); };
    axis(new T.Vector3(-R, 0, -0.04), new T.Vector3(R, 0, -0.04), '#ff6981');
    axis(new T.Vector3(0, -R, -0.04), new T.Vector3(0, R, -0.04), '#00b266');
    axis(new T.Vector3(0, 0, 0), new T.Vector3(0, 0, 40), '#7a95ff');
  }
  const PLANES = {
    XY: { o: [0, 0, 0], U: [1, 0, 0], V: [0, 1, 0] },
    XZ: { o: [0, 0, 0], U: [1, 0, 0], V: [0, 0, 1] },
    YZ: { o: [0, 0, 0], U: [0, 1, 0], V: [0, 0, 1] },
  };
  VP.PLANES = PLANES;
  function buildPlanes() {
    for (const [name, p] of Object.entries(PLANES)) {
      const o = new T.Vector3(...p.o), U = new T.Vector3(...p.U), V = new T.Vector3(...p.V);
      const s = Geo.rectShape(-4, -4, 46, 46);
      const g = new T.ShapeGeometry(s); g.applyMatrix4(new T.Matrix4().makeBasis(U, V, new T.Vector3().crossVectors(U, V)).setPosition(o));
      const m = new T.Mesh(g, new T.MeshBasicMaterial({ color: '#9f9f9f', transparent: true, opacity: 0.08, side: T.DoubleSide, depthWrite: false }));
      m.userData = { plane: name };
      const corners = [[-4, -4], [46, -4], [46, 46], [-4, 46]].map(([u, v]) => o.clone().addScaledVector(U, u).addScaledVector(V, v));
      const outline = lineMesh(corners, '#9f9f9f', 1, 0.6, true);
      m.userData.outline = outline;
      const grp = new T.Group(); grp.add(m, outline); grp.userData = { plane: name, mesh: m, outline };
      grp.visible = false;
      planeGroup.add(grp);
    }
  }
  VP.setPlanes = function (vis, hover) {
    planeGroup.children.forEach((g) => {
      const n = g.userData.plane; g.visible = !!vis[n];
      const on = hover === n;
      g.userData.mesh.material.color.set(on ? '#186add' : '#9f9f9f');
      g.userData.mesh.material.opacity = on ? 0.32 : 0.08;
      g.userData.outline.material.uniforms.uColor.value.set(on ? '#4676ff' : '#9f9f9f');
    });
    VP.request();
  };

  // ── View cube ──
  function label(text, fill) {
    const c = document.createElement('canvas'); c.width = c.height = 128; const x = c.getContext('2d');
    x.fillStyle = fill; x.fillRect(0, 0, 128, 128);
    x.strokeStyle = '#525151'; x.lineWidth = 3; x.strokeRect(1.5, 1.5, 125, 125);
    x.fillStyle = '#cfcfcf'; x.font = '500 21px "Inter Silico", Inter, sans-serif'; x.textAlign = 'center'; x.textBaseline = 'middle';
    x.fillText(text, 64, 66);
    const t = new T.CanvasTexture(c); t.anisotropy = 4; return t;
  }
  function buildCube() {
    cubeScene = new T.Scene();
    cubeCam = new T.OrthographicCamera(-1.75, 1.75, 1.75, -1.75, 0.1, 100);
    const names = [['RIGHT', '#2c2b2b'], ['LEFT', '#2c2b2b'], ['TOP', '#363535'], ['BOTTOM', '#212121'], ['FRONT', '#303030'], ['BACK', '#262626']];
    const mats = names.map(([n, f]) => new T.MeshBasicMaterial({ map: label(n, f) }));
    cube = new T.Mesh(new T.BoxGeometry(2, 2, 2), mats);
    cube.rotation.x = Math.PI / 2;
    cubeScene.add(cube);
    const eg = new T.LineSegments(new T.EdgesGeometry(new T.BoxGeometry(2, 2, 2)), new T.LineBasicMaterial({ color: '#827f7f' }));
    eg.rotation.x = Math.PI / 2; cubeScene.add(eg);
    cubePatch = new T.Mesh(new T.BufferGeometry(), new T.MeshBasicMaterial({ color: '#186add', transparent: true, opacity: 0.8, side: T.DoubleSide, depthTest: false }));
    cubePatch.visible = false; cubeScene.add(cubePatch);
    // Small axis stubs at the cube's corner, as the triad's colours.
    const ax = (d, c) => { const g = new T.BufferGeometry().setFromPoints([new T.Vector3(-1.02, -1.02, -1.02), new T.Vector3(-1.02, -1.02, -1.02).add(d)]); cubeScene.add(new T.Line(g, new T.LineBasicMaterial({ color: c }))); };
    ax(new T.Vector3(1.4, 0, 0), '#ff6981'); ax(new T.Vector3(0, 1.4, 0), '#00b266'); ax(new T.Vector3(0, 0, 1.4), '#7a95ff');
  }
  function cubeHit(cx, cy) {
    const rect = canvas.getBoundingClientRect(); const { w, h } = VP.size();
    const x0 = rect.left + w - CUBE - CUBE_M, y0 = rect.top + CUBE_M;
    if (cx < x0 || cx > x0 + CUBE || cy < y0 || cy > y0 + CUBE) return null;
    const ndc = new T.Vector2(((cx - x0) / CUBE) * 2 - 1, -((cy - y0) / CUBE) * 2 + 1);
    const rc = new T.Raycaster(); rc.setFromCamera(ndc, cubeCam);
    const hit = rc.intersectObject(cube)[0];
    if (!hit) return { region: null };
    const p = hit.point; const comps = [p.x, p.y, p.z];
    const dir = comps.map((c) => (Math.abs(c) > 0.999 ? Math.sign(c) : Math.abs(c) > 0.5 ? Math.sign(c) : 0));
    const face = comps.map((c) => (Math.abs(c) > 0.999 ? Math.sign(c) : 0));
    return { region: dir, face };
  }
  function showCubePatch(hit) {
    if (!hit || !hit.region) { cubePatch.visible = false; return; }
    const fi = hit.face.findIndex((v) => v !== 0); if (fi < 0) { cubePatch.visible = false; return; }
    const others = [0, 1, 2].filter((i) => i !== fi);
    const range = (s) => (s === 0 ? [-0.5, 0.5] : s > 0 ? [0.5, 1] : [-1, -0.5]);
    const ra = range(hit.region[others[0]]), rb = range(hit.region[others[1]]);
    const pts = [];
    for (const [a, b] of [[ra[0], rb[0]], [ra[1], rb[0]], [ra[1], rb[1]], [ra[0], rb[0]], [ra[1], rb[1]], [ra[0], rb[1]]]) {
      const p = [0, 0, 0]; p[fi] = hit.face[fi] * 1.012; p[others[0]] = a; p[others[1]] = b; pts.push(...p);
    }
    cubePatch.geometry.dispose();
    const g = new T.BufferGeometry(); g.setAttribute('position', new T.Float32BufferAttribute(pts, 3));
    cubePatch.geometry = g; cubePatch.visible = true;
  }

  // ── Model ──
  VP.setModel = function (geo) {
    model = geo;
    modelGroup.clear(); faceMeshes = []; edgeMeshes = [];
    if (!geo) { VP.request(); return; }
    for (const f of geo.faces) {
      const m = new T.Mesh(f.geom, faceMaterial()); m.userData = { face: f }; modelGroup.add(m); faceMeshes.push(m);
    }
    for (const e of geo.edges) {
      const m = lineMesh(e.pts, EDGE, 1.5, 1, false); m.userData = { edge: e }; m.renderOrder = 2; modelGroup.add(m); edgeMeshes.push(m);
    }
    applyHighlight();
  };
  VP.model = () => model;
  VP.setBodyVisible = function (v) { modelGroup.visible = v; VP.request(); };
  VP.setGridVisible = function (v) { gridGroup.visible = v; VP.request(); };
  VP.setDim = function (d) { VP.dim = d; applyHighlight(); };

  VP.setHighlight = function (hl) { Object.assign(VP.hl, hl); applyHighlight(); };
  function key(k, id) { return k + ':' + id; }
  function applyHighlight() {
    const sel = new Set(VP.hl.sel.map((s) => key(s.kind, s.id)));
    const hov = VP.hl.hover ? key(VP.hl.hover.kind, VP.hl.hover.id) : null;
    const cand = new Set(VP.hl.cand || []);
    const candHot = VP.hl.candHot;
    for (const m of faceMeshes) {
      const f = m.userData.face; const k = key('face', f.id); const u = m.material.uniforms;
      u.uDim.value = VP.dim;
      if (sel.has(k)) { u.uTint.value.copy(SELECT); u.uTintAmt.value = 0.55; }
      else if (hov === k) { u.uTint.value.copy(HOVER); u.uTintAmt.value = 0.28; }
      else if (VP.hl.maker && (VP.hl.maker === '*' || f.maker === VP.hl.maker)) { u.uTint.value.copy(MAKER); u.uTintAmt.value = 0.4; }
      else u.uTintAmt.value = 0;
    }
    for (const m of edgeMeshes) {
      const e = m.userData.edge; const k = key('edge', e.id); const u = m.material.uniforms;
      if (candHot === e.id) { u.uColor.value.copy(CAND); u.uWidth.value = 4; }
      else if (cand.has(e.id)) { u.uColor.value.copy(CAND); u.uWidth.value = 2.5; }
      else if (sel.has(k)) { u.uColor.value.copy(SELECT_EDGE); u.uWidth.value = 3.5; }
      else if (hov === k) { u.uColor.value.copy(HOVER); u.uWidth.value = 3; }
      else { u.uColor.value.copy(EDGE); u.uWidth.value = 1.5; u.uColor.value.lerp(new T.Color('#000'), 1 - VP.dim); }
      m.renderOrder = sel.has(k) || hov === k || cand.has(e.id) ? 3 : 2;
    }
    VP.request();
  }

  // ── Projection helpers ──
  VP.project = function (v) {
    updateCamera();
    const p = (v.isVector3 ? v : new T.Vector3(...v)).clone().project(camera);
    const { w, h } = VP.size();
    return { x: (p.x * 0.5 + 0.5) * w, y: (-p.y * 0.5 + 0.5) * h, z: p.z };
  };
  VP.camBasis = function () {
    const c = VP.cam; return { dir: dirOf(c.az, c.el), up: upOf(c.az, c.el), right: new T.Vector3().crossVectors(dirOf(c.az, c.el), upOf(c.az, c.el)).negate() };
  };
  VP.pxPerUnit = function () { return VP.size().h / VP.cam.height; };
  function ray(cx, cy) {
    const rect = canvas.getBoundingClientRect();
    const ndc = new T.Vector2(((cx - rect.left) / rect.width) * 2 - 1, -((cy - rect.top) / rect.height) * 2 + 1);
    updateCamera();
    const rc = new T.Raycaster(); rc.setFromCamera(ndc, camera); return rc;
  }
  VP.planeHit = function (cx, cy, frame) {
    const rc = ray(cx, cy);
    const N = new T.Vector3().crossVectors(frame.U, frame.V).normalize();
    const pl = new T.Plane().setFromNormalAndCoplanarPoint(N, frame.o);
    const p = new T.Vector3(); if (!rc.ray.intersectPlane(pl, p)) return null;
    const d = p.clone().sub(frame.o); return { u: d.dot(frame.U), v: d.dot(frame.V), p };
  };

  // ── Picking (stands in for the ID buffer; a frame late in the app) ──
  function local(cx, cy) { const r = canvas.getBoundingClientRect(); return { x: cx - r.left, y: cy - r.top }; }
  function segDist(p, a, b) {
    const dx = b.x - a.x, dy = b.y - a.y; const L = dx * dx + dy * dy;
    let t = L ? ((p.x - a.x) * dx + (p.y - a.y) * dy) / L : 0; t = Math.max(0, Math.min(1, t));
    return { d: Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy)), t };
  }
  VP.pickAll = function (cx, cy) {
    if (!model || !modelGroup.visible) return [];
    const rc = ray(cx, cy); const m = local(cx, cy);
    const hits = rc.intersectObjects(faceMeshes, false);
    const tFace = hits.length ? hits[0].distance : Infinity;
    const eps = VP.cam.height * 0.012;
    const out = [];
    const seen = new Set();
    for (const h of hits) { const f = h.object.userData.face; if (!seen.has(f.id)) { seen.add(f.id); out.push({ kind: 'face', id: f.id, t: h.distance, ref: f }); } }
    for (const e of model.edges) {
      let best = { d: Infinity };
      for (let i = 0; i < e.pts.length - 1; i++) {
        const a = VP.project(e.pts[i]), b = VP.project(e.pts[i + 1]);
        const r = segDist(m, a, b); if (r.d < best.d) best = { d: r.d, p: e.pts[i].clone().lerp(e.pts[i + 1], r.t) };
      }
      if (best.d < 6) { const t = best.p.clone().sub(rc.ray.origin).dot(rc.ray.direction); out.push({ kind: 'edge', id: e.id, t, d: best.d, visible: t <= tFace + eps, ref: e }); }
    }
    for (const v of model.verts) {
      const s = VP.project(v.p); const d = Math.hypot(s.x - m.x, s.y - m.y);
      if (d < 7) { const t = v.p.clone().sub(rc.ray.origin).dot(rc.ray.direction); out.push({ kind: 'vertex', id: v.id, t, d, visible: t <= tFace + eps, ref: v }); }
    }
    return out.sort((a, b) => a.t - b.t || (a.kind === 'face') - (b.kind === 'face'));
  };
  VP.pick = function (cx, cy) {
    const all = VP.pickAll(cx, cy); const f = VP.filter;
    const vx = all.filter((h) => h.kind === 'vertex' && h.visible).sort((a, b) => a.d - b.d)[0];
    if (vx && f.vertex) return vx;
    const ed = all.filter((h) => h.kind === 'edge' && h.visible).sort((a, b) => a.d - b.d)[0];
    if (ed && f.edge) return ed;
    const fc = all.find((h) => h.kind === 'face');
    if (fc && f.face) return fc;
    return null;
  };
  VP.boxSelect = function (r0, r1, crossing) {
    if (!model) return [];
    const x0 = Math.min(r0.x, r1.x), x1 = Math.max(r0.x, r1.x), y0 = Math.min(r0.y, r1.y), y1 = Math.max(r0.y, r1.y);
    const inside = (p) => p.x >= x0 && p.x <= x1 && p.y >= y0 && p.y <= y1;
    const out = [];
    for (const e of model.edges) {
      const ps = e.pts.map((p) => VP.project(p));
      if (crossing ? ps.some(inside) : ps.every(inside)) out.push({ kind: 'edge', id: e.id });
    }
    return out;
  };
  VP.pickPlane = function (cx, cy) {
    const rc = ray(cx, cy);
    const vis = planeGroup.children.filter((g) => g.visible).map((g) => g.userData.mesh);
    const ph = rc.intersectObjects(vis, false)[0];
    const fh = model && modelGroup.visible ? rc.intersectObjects(faceMeshes, false).find((h) => h.object.userData.face.kind === 'plane') : null;
    const fhAny = model && modelGroup.visible ? rc.intersectObjects(faceMeshes, false)[0] : null;
    if (fhAny && fhAny.object.userData.face.kind === 'plane' && (!ph || fhAny.distance <= ph.distance + 0.01)) return { face: fhAny.object.userData.face };
    if (ph) return { plane: ph.object.userData.plane };
    if (fh && !fhAny) return { face: fh.object.userData.face };
    return null;
  };

  // ── Camera moves ──
  function ease(t) { return t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2; }
  VP.animateTo = function (to, ms) {
    const from = { az: VP.cam.az, el: VP.cam.el, target: VP.cam.target.clone(), height: VP.cam.height };
    let daz = (to.az ?? from.az) - from.az; daz = ((daz + Math.PI) % (2 * Math.PI) + 2 * Math.PI) % (2 * Math.PI) - Math.PI;
    const end = { az: from.az + daz, el: to.el ?? from.el, target: to.target ? to.target.clone() : from.target, height: to.height ?? from.height };
    if (reduceMotion || ms === 0) { Object.assign(VP.cam, end); VP.request(); VP.hooks.onView && VP.hooks.onView(); return; }
    anim = { from, end, t0: performance.now(), ms: ms || 220 };
  };
  function stepAnim() {
    const k = Math.min(1, (performance.now() - anim.t0) / anim.ms); const e = ease(k); const { from, end } = anim;
    VP.cam.az = from.az + (end.az - from.az) * e; VP.cam.el = from.el + (end.el - from.el) * e;
    VP.cam.target.lerpVectors(from.target, end.target, e); VP.cam.height = from.height + (end.height - from.height) * e;
    if (k >= 1) { anim = null; VP.hooks.onView && VP.hooks.onView(); }
  }
  VP.VIEWS = {
    front: { az: 0, el: 0 }, back: { az: Math.PI, el: 0 }, left: { az: -Math.PI / 2, el: 0 }, right: { az: Math.PI / 2, el: 0 },
    top: { az: 0, el: Math.PI / 2 }, bottom: { az: 0, el: -Math.PI / 2 }, iso: { az: 45 * DEG, el: 35.264 * DEG },
  };
  VP.fitBox = function (box, view) {
    const v = view || { az: VP.cam.az, el: VP.cam.el };
    const d = dirOf(v.az, v.el), up = upOf(v.az, v.el), right = new T.Vector3().crossVectors(up, d);
    const c = box.getCenter(new T.Vector3());
    let mx = 0, my = 0;
    for (const x of [box.min.x, box.max.x]) for (const y of [box.min.y, box.max.y]) for (const z of [box.min.z, box.max.z]) {
      const p = new T.Vector3(x, y, z).sub(c); mx = Math.max(mx, Math.abs(p.dot(right))); my = Math.max(my, Math.abs(p.dot(up)));
    }
    const { w, h } = VP.size(); const aspect = w / Math.max(h, 1);
    const height = Math.max(my * 2, (mx * 2) / aspect) * 1.45 + 2;
    VP.animateTo({ az: v.az, el: v.el, target: c, height });
  };
  VP.modelBox = function () {
    if (model && modelGroup.visible) return new T.Box3().setFromObject(modelGroup);
    return new T.Box3(new T.Vector3(-10, -10, 0), new T.Vector3(50, 50, 30));
  };
  VP.fit = () => VP.fitBox(VP.modelBox());
  VP.home = () => VP.fitBox(VP.modelBox(), VP.VIEWS.iso);
  VP.view = (name) => VP.fitBox(VP.modelBox(), VP.VIEWS[name]);
  VP.viewName = function () {
    for (const [n, v] of Object.entries(VP.VIEWS)) {
      const da = Math.abs(((VP.cam.az - v.az + Math.PI) % (2 * Math.PI) + 2 * Math.PI) % (2 * Math.PI) - Math.PI);
      if (Math.abs(VP.cam.el - v.el) < 0.01 && (da < 0.01 || Math.abs(v.el) > 1.5)) return n;
    }
    return null;
  };

  // ── Input: Fusion's preset (wheel zooms to the cursor, middle pans, Shift+middle orbits) ──
  let drag = null, hoverRaf = 0, lastMove = null;
  function bindInput() {
    wrap.addEventListener('wheel', (e) => {
      e.preventDefault();
      if (VP.hooks.onWheel && VP.hooks.onWheel(e)) return;
      const f = Math.exp(e.deltaY * (e.ctrlKey ? 0.01 : 0.0015));
      zoomAt(e.clientX, e.clientY, f);
    }, { passive: false });
    wrap.addEventListener('mousedown', (e) => {
      const ch = cubeHit(e.clientX, e.clientY);
      if (ch && e.button === 0) { drag = { kind: 'cube', x: e.clientX, y: e.clientY, moved: false, hit: ch }; return; }
      if (e.button === 1 || (e.button === 0 && (e.altKey || VP.navMode))) {
        e.preventDefault();
        const orbit = e.button === 1 ? e.shiftKey : (VP.navMode === 'orbit' || (e.altKey && !e.shiftKey));
        drag = { kind: orbit ? 'orbit' : 'pan', x: e.clientX, y: e.clientY };
        wrap.classList.add('orbiting');
        return;
      }
      if (e.button === 0) {
        if (VP.hooks.onPointerDown && VP.hooks.onPointerDown(e)) { drag = { kind: 'hook', x: e.clientX, y: e.clientY }; return; }
        drag = { kind: 'press', x: e.clientX, y: e.clientY, moved: false, shift: e.shiftKey, ctrl: e.ctrlKey || e.metaKey };
      }
    });
    window.addEventListener('mousemove', (e) => {
      lastMove = e;
      if (drag) {
        const dx = e.clientX - drag.x, dy = e.clientY - drag.y;
        if (drag.kind === 'cube') { if (Math.hypot(dx, dy) > 3) drag.moved = true; if (drag.moved) { orbitBy(dx, dy); drag.x = e.clientX; drag.y = e.clientY; } return; }
        if (drag.kind === 'orbit') { orbitBy(dx, dy); drag.x = e.clientX; drag.y = e.clientY; return; }
        if (drag.kind === 'pan') { panBy(dx, dy); drag.x = e.clientX; drag.y = e.clientY; return; }
        if (drag.kind === 'hook') { VP.hooks.onPointerDrag && VP.hooks.onPointerDrag(e); return; }
        if (drag.kind === 'press') {
          if (!drag.moved && Math.hypot(dx, dy) > 4) {
            drag.moved = true;
            const onGeo = VP.mode === 'select' && VP.pick(drag.x, drag.y);
            drag.box = !onGeo && VP.mode === 'select';
          }
          if (drag.moved && drag.box) { const a = local(drag.x, drag.y), b = local(e.clientX, e.clientY); VP.boxRect = { a, b, crossing: b.x < a.x }; VP.request(); }
          return;
        }
      }
      if (e.target === canvas || wrap.contains(e.target)) {
        if (!hoverRaf) hoverRaf = requestAnimationFrame(() => { hoverRaf = 0; handleHover(lastMove); });
      }
    });
    window.addEventListener('mouseup', (e) => {
      if (!drag) return;
      const d = drag; drag = null; wrap.classList.remove('orbiting');
      if (d.kind === 'cube') {
        if (!d.moved && d.hit && d.hit.region) {
          const r = d.hit.region; const v = new T.Vector3(...r).normalize();
          let el = Math.asin(Math.max(-1, Math.min(1, v.z)));
          let az = Math.abs(v.z) > 0.999 ? 0 : Math.atan2(v.x, -v.y);
          VP.fitBox(VP.modelBox(), { az, el });
        }
        return;
      }
      if (d.kind === 'hook') { VP.hooks.onPointerUp && VP.hooks.onPointerUp(e); return; }
      if (d.kind !== 'press') return;
      if (d.moved && d.box && VP.boxRect) {
        const br = VP.boxRect; VP.boxRect = null; VP.request();
        VP.hooks.onBox && VP.hooks.onBox(VP.boxSelect(br.a, br.b, br.crossing), { add: e.shiftKey, invert: e.ctrlKey || e.metaKey, crossing: br.crossing });
        return;
      }
      if (!d.moved) VP.hooks.onClick && VP.hooks.onClick(e);
    });
    wrap.addEventListener('dblclick', (e) => { if (!cubeHit(e.clientX, e.clientY)) VP.hooks.onDblClick && VP.hooks.onDblClick(e); });
    wrap.addEventListener('auxclick', (e) => { if (e.button === 1) e.preventDefault(); });
    wrap.addEventListener('contextmenu', (e) => { e.preventDefault(); if (cubeHit(e.clientX, e.clientY)) { VP.hooks.onCubeMenu && VP.hooks.onCubeMenu(e); return; } VP.hooks.onContext && VP.hooks.onContext(e); });
    wrap.addEventListener('mouseleave', () => { if (cubeHover) { cubeHover = null; showCubePatch(null); VP.request(); } VP.hooks.onLeave && VP.hooks.onLeave(); });
  }
  function handleHover(e) {
    if (!e) return;
    const ch = cubeHit(e.clientX, e.clientY);
    const was = JSON.stringify(cubeHover && cubeHover.region);
    cubeHover = ch;
    if (JSON.stringify(ch && ch.region) !== was) { showCubePatch(ch); VP.request(); }
    if (ch) { VP.hooks.onHover && VP.hooks.onHover(null, e); return; }
    VP.hooks.onHover && VP.hooks.onHover(e);
  }
  function zoomAt(cx, cy, f) {
    const m = local(cx, cy); const { w, h } = VP.size();
    const nx = (m.x / w) * 2 - 1, ny = -((m.y / h) * 2 - 1);
    const B = VP.camBasis(); const hh = VP.cam.height / 2, hw = hh * (w / h);
    const p = VP.cam.target.clone().addScaledVector(B.right, nx * hw).addScaledVector(B.up, ny * hh);
    VP.cam.height = Math.max(2, Math.min(4000, VP.cam.height * f));
    const hh2 = VP.cam.height / 2, hw2 = hh2 * (w / h);
    VP.cam.target.copy(p).addScaledVector(B.right, -nx * hw2).addScaledVector(B.up, -ny * hh2);
    VP.request(); VP.hooks.onView && VP.hooks.onView();
  }
  VP.zoomBy = (f) => { const r = canvas.getBoundingClientRect(); zoomAt(r.left + r.width / 2, r.top + r.height / 2, f); };
  function orbitBy(dx, dy) {
    VP.cam.az -= dx * 0.45 * DEG; VP.cam.el = Math.max(-89.9 * DEG, Math.min(89.9 * DEG, VP.cam.el + dy * 0.45 * DEG));
    VP.request(); VP.hooks.onView && VP.hooks.onView();
  }
  function panBy(dx, dy) {
    const B = VP.camBasis(); const s = VP.cam.height / VP.size().h;
    VP.cam.target.addScaledVector(B.right, -dx * s).addScaledVector(B.up, dy * s);
    VP.request(); VP.hooks.onView && VP.hooks.onView();
  }

  // ── Overlay (egui's painter over the viewport: triad, box, and the app's own shapes) ──
  function drawOverlay() {
    const { w, h } = VP.size();
    let s = '';
    // Triad, bottom left
    const B = VP.camBasis(); const ox = 44, oy = h - 44, L = 26;
    const axes = [['X', new T.Vector3(1, 0, 0), '#ff6981'], ['Y', new T.Vector3(0, 1, 0), '#00b266'], ['Z', new T.Vector3(0, 0, 1), '#7a95ff']];
    axes.sort((a, b) => a[1].dot(B.dir) - b[1].dot(B.dir));
    for (const [n, v, c] of axes) {
      const x = ox + v.dot(B.right) * L, y = oy - v.dot(B.up) * L;
      s += `<line x1="${ox}" y1="${oy}" x2="${x.toFixed(1)}" y2="${y.toFixed(1)}" stroke="${c}" stroke-width="2" stroke-linecap="round"/>`;
      const lx = ox + v.dot(B.right) * (L + 9), ly = oy - v.dot(B.up) * (L + 9);
      s += `<text x="${lx.toFixed(1)}" y="${(ly + 3.5).toFixed(1)}" fill="${c}" font-size="10.5" text-anchor="middle" font-family="Inter Silico, Inter">${n}</text>`;
    }
    s += `<circle cx="${ox}" cy="${oy}" r="2" fill="#cfcfcf"/>`;
    if (VP.boxRect) {
      const { a, b, crossing } = VP.boxRect;
      const x = Math.min(a.x, b.x), y = Math.min(a.y, b.y), bw = Math.abs(a.x - b.x), bh = Math.abs(a.y - b.y);
      s += `<rect x="${x}" y="${y}" width="${bw}" height="${bh}" fill="rgba(24,106,221,0.14)" stroke="#4676ff" stroke-width="1" ${crossing ? 'stroke-dasharray="4 3"' : ''}/>`;
    }
    if (VP.hooks.overlay) s += VP.hooks.overlay(w, h);
    overlay.innerHTML = s;
  }
  VP.redrawOverlay = () => VP.request();

  // ── Thumbnails for the start page (rendered once, as the app would cache them) ──
  VP.thumb = function (kind, w, h) {
    // Transparent, over the thumbnail's own gradient (styles.css's .thumb and .sthumb).
    const r = new T.WebGLRenderer({ antialias: true, preserveDrawingBuffer: true, alpha: true });
    r.setPixelRatio(2); r.setSize(w, h, false); r.setClearColor(0x000000, 0);
    const sc = new T.Scene(); let geo, edgesG;
    const grp = new T.Group();
    if (kind === 'bracket') {
      const b = Geo.buildBracket({ W: 60, H: 50, tb: 5, tu: 5, D: 40, r: 5, baseHoles: true, uprightHoles: true });
      b.faces.forEach((f) => grp.add(new T.Mesh(f.geom, faceMaterial())));
      b.edges.forEach((e) => { const m = lineMesh(e.pts, EDGE, 1.2, 1); m.material.uniforms.uRes.value.set(w * 2, h * 2); grp.add(m); });
    } else {
      geo = kind === 'plate' ? Geo.plateGeometry() : Geo.chamferBoxGeometry();
      grp.add(new T.Mesh(geo, faceMaterial()));
      edgesG = new T.EdgesGeometry(geo, 25);
      grp.add(new T.LineSegments(edgesG, new T.LineBasicMaterial({ color: '#101010' })));
    }
    sc.add(grp);
    const box = new T.Box3().setFromObject(grp); const c = box.getCenter(new T.Vector3());
    const v = { az: 40 * DEG, el: 28 * DEG }; const d = dirOf(v.az, v.el), up = upOf(v.az, v.el);
    const size = box.getSize(new T.Vector3()).length();
    const cam = new T.OrthographicCamera(-size * 0.62 * (w / h), size * 0.62 * (w / h), size * 0.62, -size * 0.62, 0.1, 5000);
    cam.position.copy(c).addScaledVector(d, 1000); cam.up.copy(up); cam.lookAt(c); cam.updateMatrixWorld();
    r.render(sc, cam);
    const url = r.domElement.toDataURL('image/png');
    r.dispose();
    return url;
  };

  // A line mesh for sketches drawn in 3D (kept for the browser's visible sketches).
  VP.setSketchLines = function (lines) {
    sketchGroup.clear();
    for (const l of lines || []) { const m = lineMesh(l.pts, l.color, l.width || 1.5, 1, l.closed); m.renderOrder = 4; sketchGroup.add(m); }
    VP.request();
  };

  window.VP = VP;
})();
