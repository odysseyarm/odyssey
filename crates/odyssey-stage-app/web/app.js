'use strict';
/* Odyssey Stage UI.
 * Data flow: GET /api/state fills S (stage, derived, presets, hub). A user edit calls edit(fn): fn mutates S.draft (a
 * clone of the last accepted stage), views update optimistically, and a throttled PUT /api/stage returns the accepted
 * stage + derived data (or a 400 message, shown next to the control while the last good stage stays on screen).
 * SSE: "frame" (~30 Hz) is kept as the latest string and painted once per animation frame into existing SVG nodes.
 * "hub" (~3 Hz) updates S.hub, "press" flashes a glyph.
 * Panels are "zones": built once (or rebuilt only when their signature changes) plus sync functions that write values
 * in place and never touch the focused control. SVG views are rebuilt on stage changes by buildTop()/buildDisplay().
 */
const NS = 'http://www.w3.org/2000/svg';
const SVG_TAGS = new Set(['svg', 'g', 'path', 'circle', 'line', 'rect', 'polyline', 'polygon', 'text', 'title']);
const PALETTE = ['#4cc9f0', '#f72585', '#ffd166', '#06d6a0', '#b388ff', '#ff9f1c', '#90be6d', '#e76f51', '#a0c4ff', '#fb6f92'];
const DASH = { strong: '', fair: '9 4', marginal: '4 4', lost: '2 5' };
const QUALITY_TEXT = { strong: 'Strong', fair: 'Fair', marginal: 'Marginal', lost: 'Lost' };
const DISPLAY_INFO = {
  monitor: ['Monitor', 'Flat desk monitor'], 'tv-55': ['55-inch TV', 'Flat TV'],
  'projector-wall': ['Projector on a wall', 'Slightly skewed picture'],
  'pull-down-screen': ['Pull-down screen', 'Gently wavy'], 'tarp-on-a-wall': ['Tarp on a wall', 'Very wavy and skewed'],
};
const PATTERN_INFO = {
  hold: 'Hold still', circle: 'Circle', 'figure-eight': 'Figure eight', raster: 'Scan rows', corners: 'Four corners', 'nine-point': 'Nine points',
};
const ENV_INFO = { open: 'Open space', furnished: 'Furnished room', cluttered: 'Cluttered room' };
const TYPE_PRESET = { hold: 'hold', circle: 'circle', lissajous: 'figure-eight', raster: 'raster', tour: 'corners' };

const S = {
  stage: null, derived: null, presets: null, draft: null, selected: null, frame: null, frameRaw: null, lastFrameAt: performance.now(),
  hub: { connected: false, error: null, devices: [], sim: null, screen: null }, hubDirty: true,
  detailed: localStorage.getItem('odyssey.detailed') === '1', shade: true, tool: null,
  drag: false, dragId: null, sliding: false, scrub: false, dirty: true, conn: true,
  trails: {}, ver: 0,
  eval: { device: null, ab: true, busy: false, msg: '', error: null, runs: [], stage: '', token: 0, ver: 0 },
  blender: { busy: false, error: null, data: null, ver: 0 },
};
const T = { V: null, gl: {}, links: {}, live: {}, liveSig: '' };   // top-down view nodes
const D = { V: null, cross: {}, live: {} };                         // display view nodes
const cur = () => S.draft || S.stage;
const $ = (q, r = document) => r.querySelector(q);
const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
const rnd = (v, d = 2) => (Number.isFinite(v) ? +v.toFixed(d) : '–');
const colorOf = (id) => PALETTE[Math.max(0, cur().devices.findIndex((d) => d.id === id)) % PALETTE.length];
const sel = () => cur().devices.find((d) => d.id === S.selected);
const trackers = () => cur().devices.filter((d) => d.kind === 'tracker');
const realDevices = () => S.hub.devices.filter((d) => !d.simulated);
const markDirty = () => { S.dirty = true; };

function mk(tag, attrs = {}, ...kids) {
  const e = SVG_TAGS.has(tag) ? document.createElementNS(NS, tag) : document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v === true ? '' : v);
  }
  for (const c of kids.flat()) if (c != null && c !== false) e.append(c);
  return e;
}

async function api(method, path, body) {
  let r;
  try {
    r = await fetch(path, method === 'GET' ? {} : {
      method, headers: { 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : typeof body === 'string' ? body : JSON.stringify(body),
    });
  } catch { throw new Error('Cannot reach the Odyssey Stage server.'); }
  if (r.status === 204) return null;
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(j.error || `Request failed (${r.status})`);
  return j;
}

function setError(sec, msg) {
  const b = $(`.banner[data-sec="${sec}"]`) || $('.banner[data-sec="top"]');
  b.textContent = msg || '';
  b.hidden = !msg;
}
const fail = (sec) => (e) => setError(sec, e.message);

/* ---------- zones: panels with in-place value sync ---------- */
const zones = [];
let Z = null;
function zone(root, sig, build) {
  root.append = (...kids) => Element.prototype.append.apply(root, kids.flat().filter((c) => c != null && c !== false));
  zones.push({ root, sig, build, cur: null, syncs: [] });
}
function sync(fn) { Z.syncs.push(fn); fn(); }
function rebuild(z, sig) {
  const act = document.activeElement;
  const items = () => [...z.root.querySelectorAll('input,select,button')];
  const idx = z.root.contains(act) ? items().indexOf(act) : -1;
  z.syncs = []; Z = z; z.root.replaceChildren(); z.build(z.root); Z = null; z.cur = sig;
  if (idx >= 0) items()[idx]?.focus();
}
function refresh() {
  if (!S.stage) return;
  for (const z of zones) {
    const sig = z.sig();
    if (sig !== z.cur && !(S.sliding && z.root.contains(document.activeElement))) rebuild(z, sig);
    for (const f of z.syncs) { try { f(); } catch (e) { console.error(e); } }
  }
  syncBar();
}

/* ---------- editing the stage ---------- */
let putTimer = 0, putBusy = false, seq = 0;
function edit(fn, sec = 'top', now = false) {
  S.draft ||= structuredClone(S.stage);
  fn(S.draft);
  ensureSelection(); seq++;
  setError(sec, '');
  S.errSec = sec;
  markDirty(); refresh();
  if (now) { clearTimeout(putTimer); putTimer = 0; }
  if (!putTimer && !putBusy) putTimer = setTimeout(flush, now ? 0 : 120);
}
async function flush() {
  putTimer = 0;
  if (!S.draft) return;
  putBusy = true;
  const mine = seq;
  try {
    const r = await api('PUT', '/api/stage', S.draft);
    S.stage = r.stage; S.derived = r.derived; S.ver++;
    if (mine === seq) S.draft = null;
    S.error = null;
  } catch (e) { S.error = e.message; setError(S.errSec, e.message); }
  putBusy = false;
  if (seq !== mine) putTimer = setTimeout(flush, 0);
  ensureSelection(); markDirty(); refresh();
}
async function flushNow() {
  clearTimeout(putTimer); putTimer = 0;
  if (S.draft && !putBusy) await flush();
}
function applyReply(r) {
  S.stage = r.stage; S.derived = r.derived; S.draft = null; S.error = null; seq++; S.ver++;
  ensureSelection(); markDirty(); refresh();
}
function settle() {
  if (S.error && !putTimer && !putBusy) { S.draft = null; S.error = null; markDirty(); refresh(); }
}
function ensureSelection() {
  const ds = cur().devices;
  if (!ds.some((d) => d.id === S.selected)) S.selected = (ds.find((d) => d.kind === 'tracker') || ds[0])?.id ?? null;
}
function selectDevice(id) {
  S.selected = id;
  for (const [k, v] of Object.entries(T.gl)) v.g.classList.toggle('sel', k === id);
  refresh(); updateTicks();
}
const run = (body) => api('POST', '/api/run', body).catch(fail('top'));

/* ---------- form controls (register sync functions in the current zone) ---------- */
function num(label, acc, o = {}) {
  const input = mk('input', { type: 'number', min: o.min, max: o.max, step: o.step ?? 'any', title: o.title || label });
  const k = o.scale ?? 1;
  input.addEventListener('input', () => {
    const v = input.valueAsNumber;
    if (Number.isFinite(v)) edit((d) => { const [c, key] = acc(d); if (c) c[key] = v / k; }, o.sec);
  });
  sync(() => {
    const [c, key] = acc(cur());
    if (c && document.activeElement !== input) input.value = rnd(c[key] * k, 4);
    input.disabled = !!(o.off && o.off());
  });
  return mk('label', { class: 'f', title: o.title }, mk('span', {}, label), input, o.unit && mk('em', {}, o.unit));
}
function slide(label, acc, o) {
  const input = mk('input', { type: 'range', min: o.min, max: o.max, step: o.step, id: `s${++S.ver}${Math.random().toString(36).slice(2, 6)}` });
  const out = mk('output', { for: input.id });
  const k = o.scale ?? 1;
  const show = (v) => { out.textContent = (o.fmt ? o.fmt(v * k) : rnd(v * k, 2)); };
  input.addEventListener('input', () => {
    show(+input.value / k);
    edit((d) => { const [c, key] = acc(d); if (c) c[key] = +input.value / k; }, o.sec);
  });
  sync(() => {
    const [c, key] = acc(cur());
    if (!c) return;
    if (document.activeElement !== input) input.value = c[key] * k;
    show(c[key]);
    input.disabled = !!(o.off && o.off());
  });
  return mk('div', { class: 'slider', title: o.hint }, mk('label', { for: input.id }, label), out, input, o.hint && mk('small', {}, o.hint));
}
function choice(label, options, get, set, o = {}) {
  const s = mk('select', { title: o.title || label });
  for (const [v, t, off, tip] of options) s.append(mk('option', { value: v, disabled: off, title: tip }, t));
  s.addEventListener('change', () => set(s.value));
  sync(() => { if (document.activeElement !== s) s.value = get(); });
  return mk('label', { class: 'f', title: o.title }, mk('span', {}, label), s);
}
const btn = (text, onclick, o = {}) => mk('button', { type: 'button', onclick, title: o.title, class: o.cls, disabled: o.off }, text);
function toggleBtn(text, onclick, active, o = {}) {
  const b = btn(text, onclick, o);
  sync(() => b.setAttribute('aria-pressed', !!active()));
  return b;
}
function text(get, tag = 'span', cls) {
  const e = mk(tag, { class: cls });
  sync(() => { const v = get(); if (e.textContent !== v) e.textContent = v; });
  return e;
}
function copyText(str, button) {
  const done = () => { const old = button.textContent; button.textContent = 'Copied'; setTimeout(() => { button.textContent = old; }, 1200); };
  if (navigator.clipboard?.writeText) navigator.clipboard.writeText(str).then(done, () => fallbackCopy(str, done));
  else fallbackCopy(str, done);
}
function fallbackCopy(str, done) {
  const t = mk('textarea', { style: 'position:fixed;opacity:0' }); t.value = str; document.body.append(t); t.select();
  try { document.execCommand('copy'); done(); } catch { /* user can select the text by hand */ }
  t.remove();
}

function applyDisplay(d, disp) {
  const fx = disp.width_m / d.display.width_m, fy = disp.height_m / d.display.height_m, r = (v) => Math.round(v * 100) / 100;
  const scale = (p) => { p[0] = r(p[0] * fx); p[1] = r(p[1] * fy); p[2] = Math.min(-0.3, r(p[2] * fx)); };
  for (const dev of d.devices) { scale(dev.position_m); if (dev.movement?.type === 'walk') scale(dev.movement.to); }
  d.display = disp;
}
/* ---------- Display panel ---------- */
const flatShape = (d) => JSON.stringify({ ...d, imported: undefined, width_m: 0, height_m: 0 });
zone($('#z-display'), () => 1, (root) => {
  const names = Object.keys(S.presets.displays).sort((a, b) => (Object.keys(DISPLAY_INFO).indexOf(a) + 99) % 99 - (Object.keys(DISPLAY_INFO).indexOf(b) + 99) % 99);
  const cards = mk('div', { class: 'cards' });
  for (const n of names) {
    const p = S.presets.displays[n], [title, sub] = DISPLAY_INFO[n] || [n, ''];
    const b = mk('button', { type: 'button', title: `${title}: ${sub}. Replaces the display; devices keep their place relative to its size.`, onclick: () => edit((d) => applyDisplay(d, structuredClone(p)), 'display', true) },
      title, mk('small', {}, `${rnd(p.width_m)} × ${rnd(p.height_m)} m · ${sub}`));
    sync(() => b.setAttribute('aria-pressed', !cur().display.imported && cur().display.width_m === p.width_m && cur().display.height_m === p.height_m && flatShape(cur().display) === flatShape(p)));
    cards.append(b);
  }
  const imp = mk('div', { class: 'note warn' }, text(() => { const s = S.derived.surface, hub = cur().display.imported?.hub_screen != null; return `${hub ? 'This is the hub’s own display' : 'Display imported from a scene file'} (${s ? `${s.columns}×${s.rows} grid` : 'flat'}). `; }),
    btn('Back to adjustable display', () => edit((d) => { delete d.display.imported; }, 'display', true), { title: 'Forget the imported shape and use the sliders again' }));
  sync(() => { imp.hidden = !cur().display.imported; });
  const off = () => !!cur().display.imported;
  const sized = (k) => (d) => [{ get v() { return d.display[k]; }, set v(x) { if (x >= 0.1) applyDisplay(d, { ...d.display, [k]: x }); } }, 'v'];
  const w = sized('width_m'), h = sized('height_m');
  const keys = (k) => (d) => [d.display.waviness, k], ks = (k) => (d) => [d.display.keystone, k];
  root.append(cards, imp,
    mk('div', { class: 'grid2' },
      num('Width', w, { min: 0.1, max: 30, step: 0.05, unit: 'm', sec: 'display', title: 'Display width in meters (picture area)' }),
      num('Height', h, { min: 0.1, max: 30, step: 0.05, unit: 'm', sec: 'display', title: 'Display height in meters (picture area)' })),
    mk('div', { class: 'detailed-only' },
      mk('div', { class: 'sub' }, 'Surface shape'),
      slide('Ripple height', keys('amplitude_m'), { min: 0, max: 100, step: 1, scale: 1000, fmt: (v) => `${rnd(v, 0)} mm`, sec: 'display', off, hint: 'How far the surface bulges toward or away from you. 0 = perfectly flat.' }),
      slide('Ripples across', keys('waves_u'), { min: 0, max: 6, step: 0.5, sec: 'display', off, hint: 'How many ripples from left to right.' }),
      slide('Ripple bumps down', keys('waves_v'), { min: 1, max: 4, step: 1, sec: 'display', off, hint: 'Bumps from top to bottom. The screen stays flat at the top and bottom edges where the markers sit.' }),
      slide('Ripple phase', keys('phase_deg'), { min: 0, max: 360, step: 5, fmt: (v) => `${rnd(v, 0)}°`, sec: 'display', off, hint: 'Slides the ripples sideways without changing them.' }),
      mk('div', { class: 'sub' }, 'Projector placement'),
      slide('Keystone, horizontal', ks('horizontal'), { min: -0.3, max: 0.3, step: 0.01, scale: 100, fmt: (v) => `${rnd(v, 0)}%`, sec: 'display', off, hint: 'Positive: top edge wider than the bottom (projector aimed too high).' }),
      slide('Keystone, vertical', ks('vertical'), { min: -0.3, max: 0.3, step: 0.01, scale: 100, fmt: (v) => `${rnd(v, 0)}%`, sec: 'display', off, hint: 'Positive: left edge taller than the right.' }),
      slide('Barrel / pincushion', (d) => [d.display, 'barrel'], { min: -0.3, max: 0.3, step: 0.01, scale: 100, fmt: (v) => `${rnd(v, 0)}%`, sec: 'display', off, hint: 'Lens distortion. Positive bulges the picture outward at the edges; negative pinches it.' })));
});

/* ---------- Devices panel ---------- */
function newId(kind) {
  let n = 1;
  while (cur().devices.some((d) => d.id === `${kind}-${n}`)) n++;
  return [`${kind}-${n}`, n];
}
function addDevice(kind) {
  const [id, n] = newId(kind), W = cur().display.width_m, H = cur().display.height_m;
  const dev = kind === 'tracker'
    ? { id, name: `Tracker ${n}`, kind, binding: { type: 'stage' }, position_m: [rnd(W / 2), rnd(H / 2), -2], pattern: structuredClone(S.presets.patterns.circle || { type: 'hold', at: [0.5, 0.5] }),
        handling: { tremor_deg: 0, tremor_hz: 9, drift_deg: 0, recoil_deg: 0, seed: 1 }, movement: { type: 'still' }, presses: [], roll_deg: 0 }
    : { id, name: `Dongle ${n}`, kind, binding: { type: 'stage' }, position_m: [rnd(W + 0.5), rnd(H / 2), -0.5] };
  edit((d) => { d.devices.push(dev); }, 'devices', true);
  selectDevice(id);
}
function removeDevice(dev) {
  if (confirm(`Delete "${dev.name}"?`)) edit((d) => { d.devices = d.devices.filter((x) => x.id !== dev.id); }, 'devices', true);
}
zone($('#z-devices'), () => JSON.stringify([S.selected, cur().devices.map((d) => [d.id, d.name, d.kind])]), (root) => {
  const list = mk('div', { role: 'list' });
  cur().devices.forEach((dev) => {
    const row = mk('div', { class: `dev-row${dev.id === S.selected ? ' sel' : ''}`, role: 'listitem' },
      mk('span', { class: 'sw', style: `background:${colorOf(dev.id)}` }),
      mk('span', { class: 'ico', title: dev.kind === 'tracker' ? 'Tracker' : 'Dongle (radio receiver)', 'aria-hidden': true }, dev.kind === 'tracker' ? '◎' : '◉'),
      mk('button', { type: 'button', class: 'name', title: `Select ${dev.name}`, onclick: () => selectDevice(dev.id) }, dev.name, mk('span', { class: 'muted' }, ` (${dev.kind})`)),
      mk('button', { type: 'button', class: 'tiny', title: `Rename ${dev.name}`, 'aria-label': `Rename ${dev.name}`, onclick: () => { selectDevice(dev.id); $('#z-inspector input')?.focus(); } }, '✎'),
      mk('button', { type: 'button', class: 'tiny danger', title: `Delete ${dev.name}`, 'aria-label': `Delete ${dev.name}`, onclick: () => removeDevice(dev) }, '✕'));
    list.append(row);
  });
  root.append(list,
    mk('div', { class: 'row' }, btn('+ Tracker', () => addDevice('tracker'), { title: 'Add a tracker (the thing you aim). Placed 2 m in front of the display.' }),
      btn('+ Dongle', () => addDevice('dongle'), { title: 'Add a dongle (radio receiver). Placed beside the display.' })),
    !trackers().length && mk('div', { class: 'note' }, 'Add a tracker to see something move.'));
});

const devAcc = (id, ...path) => (d) => {
  let c = d.devices.find((x) => x.id === id);
  for (const p of path.slice(0, -1)) c = c?.[p];
  return [c, path[path.length - 1]];
};
function defaultMovement(type, pos) {
  if (type === 'walk') return { type, to: [rnd(pos[0] + 1), pos[1], pos[2]], period_s: 8 };
  if (type === 'orbit') return { type, radius_m: 0.5, period_s: 10 };
  return { type: 'still' };
}
function patternFields(id, pat) {
  const A = (...p) => devAcc(id, 'pattern', ...p), o = { min: 0, max: 1, step: 0.05, sec: 'devices' };
  const pair = (label, key, ou) => mk('div', { class: 'grid2' }, num(`${label} left–right`, A(key, 0), ou || o), num(`${label} top–bottom`, A(key, 1), ou || o));
  const period = num('Time for one loop', A('period_s'), { min: 0.5, step: 0.5, unit: 's', sec: 'devices' });
  switch (pat.type) {
    case 'hold': return [pair('Aim at', 'at')];
    case 'circle': return [pair('Center', 'center'), pair('Radius', 'radius'), period];
    case 'lissajous': return [pair('Center', 'center'), pair('Size', 'amplitude'), pair('Cycles', 'frequency', { min: 0, max: 8, step: 1, sec: 'devices' }),
      num('Phase', A('phase_deg'), { step: 5, unit: '°', sec: 'devices' }), period];
    case 'raster': return [num('Rows', A('rows'), { min: 1, max: 50, step: 1, sec: 'devices' }), num('Margin', A('margin'), { min: 0, max: 0.45, step: 0.05, sec: 'devices', title: 'Distance kept from the display edge (fraction)' }), period];
    default: return [
      ...pat.points.map((_, i) => mk('div', { class: 'row' }, mk('span', { class: 'muted' }, `Point ${i + 1}`), num('left–right', A('points', i, 0), o), num('top–bottom', A('points', i, 1), o),
        btn('✕', () => edit((d) => { devAcc(id, 'pattern')(d)[0].points.splice(i, 1); }, 'devices', true), { title: 'Remove this point', cls: 'tiny danger', off: pat.points.length < 2 }))),
      btn('+ Add point', () => edit((d) => { devAcc(id, 'pattern')(d)[0].points.push([0.5, 0.5]); }, 'devices', true), { title: 'Add another stop' }), period];
  }
}
function bindingControls(dev) {
  const id = dev.id, tracker = dev.kind === 'tracker';
  const real = realDevices();
  const opts = [['stage', 'Stage (simulated here)'],
    ['hub_sim', 'Hub simulator', !S.hub.sim, !S.hub.sim ? 'Needs a hub started with --sim' : tracker ? 'Becomes a simulated tracker of the running hub' : 'Becomes a simulated dongle of the running hub; trackers bound to the hub connect through it']];
  if (tracker) opts.push(['real', 'A real tracker', !real.length && dev.binding.type !== 'real', real.length ? 'Show a physical tracker the hub reports' : 'No physical tracker reported by a hub']);
  const out = [choice('Driven by', opts, () => sel()?.binding.type, (v) => edit((d) => {
    d.devices.find((x) => x.id === id).binding = v === 'real' ? { type: 'real', uuid: real[0]?.uuid || '' } : { type: v };
  }, 'devices', true), { title: tracker ? 'Who controls this tracker: this stage, the hub’s simulator, or a physical tracker' : 'Whether this dongle exists only here or also in the hub’s simulator' })];
  if (dev.binding.type === 'real') {
    const ropts = real.map((r) => [r.uuid, r.name || r.uuid]);
    if (!real.some((r) => r.uuid === dev.binding.uuid)) ropts.push([dev.binding.uuid, `${dev.binding.uuid || '(none)'} (not connected)`]);
    out.push(choice('Physical tracker', ropts, () => sel()?.binding.uuid, (v) => edit((d) => { d.devices.find((x) => x.id === id).binding.uuid = v; }, 'devices', true)),
      mk('div', { class: 'note' }, 'Shows the physical tracker the hub reports; the aim pattern is ignored.'));
  }
  return out;
}
zone($('#z-inspector'), () => {
  const d = sel();
  return d ? JSON.stringify([d.id, d.kind, d.pattern?.type, d.pattern?.points?.length, d.movement?.type, d.presses?.length, d.binding.type, realDevices().map((r) => [r.uuid, r.name]), !!S.hub.sim, S.stage.devices.map((x) => x.binding.type)]) : 'none';
}, (root) => {
  const dev = sel();
  if (!dev) return;
  const id = dev.id, sec = 'devices', P = (i) => devAcc(id, 'position_m', i);
  const nameIn = mk('input', { type: 'text', maxlength: 60, title: 'Name shown in the lists and views' });
  nameIn.addEventListener('input', () => edit((d) => { devAcc(id, 'name')(d)[0].name = nameIn.value; }, sec));
  sync(() => { if (document.activeElement !== nameIn) nameIn.value = sel()?.name ?? ''; });
  const where = mk('div', { class: 'grid2' },
    num('Across', P(0), { step: 0.05, unit: 'm', sec, title: 'Distance from the display’s left edge, in meters' }),
    num('Down', P(1), { step: 0.05, unit: 'm', sec, title: 'Distance below the display’s top edge, in meters' }),
    num('In front', (d) => { const x = d.devices.find((v) => v.id === id); return [{ get z() { return -x.position_m[2]; }, set z(v) { x.position_m[2] = -v; } }, 'z']; },
      { step: 0.05, min: 0.3, unit: 'm', sec, title: 'Distance in front of the display, in meters (at least 0.3)' }));
  root.append(mk('div', { class: 'sub' }, dev.kind === 'tracker' ? 'Selected tracker' : 'Selected dongle'),
    mk('label', { class: 'f' }, mk('span', {}, 'Name'), nameIn), where,
    mk('div', { class: 'detailed-only' }, ...bindingControls(dev)));
  if (dev.kind !== 'tracker') { root.append(mk('div', { class: 'note' }, 'A dongle receives the radio signals. Drag it in the room view and watch the link colours.')); return; }
  const chips = mk('div', { class: 'chips' });
  for (const [n, p] of Object.entries(S.presets.patterns)) {
    const b = btn(PATTERN_INFO[n] || n, () => edit((d) => { devAcc(id, 'pattern')(d)[0].pattern = structuredClone(p); }, sec, true), { title: `Aim pattern: ${PATTERN_INFO[n] || n}` });
    sync(() => b.setAttribute('aria-pressed', JSON.stringify(sel()?.pattern) === JSON.stringify(p)));
    chips.append(b);
  }
  const typeOpts = [['hold', 'Hold still'], ['circle', 'Circle'], ['lissajous', 'Lissajous (figure eight…)'], ['raster', 'Scan rows'], ['tour', 'Tour of points']];
  const setType = (t) => edit((d) => { devAcc(id, 'pattern')(d)[0].pattern = structuredClone(S.presets.patterns[TYPE_PRESET[t]]); }, sec, true);
  const moveSet = (t) => edit((d) => { const x = d.devices.find((v) => v.id === id); x.movement = defaultMovement(t, x.position_m); }, sec, true);
  const M = (...p) => devAcc(id, 'movement', ...p), H = (k) => devAcc(id, 'handling', k);
  const hs = (label, k, max, unit, hint) => slide(label, H(k), { min: 0, max, step: 0.1, fmt: (v) => `${rnd(v, 1)} ${unit}`, sec, hint });
  root.append(
    mk('div', { class: 'sub' }, 'Where it aims'), chips,
    mk('div', { class: 'detailed-only' },
      choice('Pattern type', typeOpts, () => sel()?.pattern.type, setType, { title: 'The kind of path the tracker aims along' }), ...patternFields(id, dev.pattern),
      mk('div', { class: 'sub' }, 'Movement'),
      choice('Tracker moves', [['still', 'Stays put'], ['walk', 'Walks back and forth'], ['orbit', 'Circles around']], () => sel()?.movement.type, moveSet),
      ...(dev.movement.type === 'walk' ? [mk('div', { class: 'grid2' }, num('To across', M('to', 0), { step: 0.1, unit: 'm', sec }), num('To down', M('to', 1), { step: 0.1, unit: 'm', sec }), num('To z', M('to', 2), { step: 0.1, max: -0.3, unit: 'm', sec, title: 'Negative = in front of the display' }), num('Loop time', M('period_s'), { min: 0.5, unit: 's', sec }))]
        : dev.movement.type === 'orbit' ? [mk('div', { class: 'grid2' }, num('Radius', M('radius_m'), { min: 0, step: 0.1, unit: 'm', sec }), num('Loop time', M('period_s'), { min: 0.5, unit: 's', sec }))] : []),
      mk('div', { class: 'sub' }, 'Hand handling'),
      hs('Hand shake', 'tremor_deg', 10, '°', 'Small fast trembling of the aim.'), hs('Shake speed', 'tremor_hz', 30, 'Hz', 'Real hands shake around 8–12 Hz.'),
      hs('Slow drift', 'drift_deg', 10, '°', 'Slow wandering of the aim.'), hs('Recoil', 'recoil_deg', 10, '°', 'Upward kick on every trigger pull.'),
      num('Random seed', H('seed'), { step: 1, min: 0, sec, title: 'Same seed = same shake every run' }),
      num('Roll', devAcc(id, 'roll_deg'), { step: 5, unit: '°', sec, title: 'Twist of the tracker around its aiming axis' })),
    mk('div', { class: 'sub' }, 'Buttons'),
    mk('div', { class: 'row' },
      btn('Pull trigger now', () => api('POST', '/api/press', { device: id, button: 'trigger' }).catch(fail(sec)), { title: 'Press the trigger once, right now' }),
      btn('Pair', () => api('POST', '/api/press', { device: id, button: 'pair' }).catch(fail(sec)), { title: 'Press the pairing button once, right now' })),
    mk('div', { class: 'detailed-only' }, pressTable(dev)));
});
function pressTable(dev) {
  const id = dev.id, sec = 'devices', body = mk('tbody');
  dev.presses.forEach((p, i) => {
    body.append(mk('tr', {}, mk('td', {}, num('At', devAcc(id, 'presses', i, 'at_s'), { min: 0, step: 0.1, unit: 's', sec })),
      mk('td', {}, choice('Button', [['trigger', 'Trigger'], ['pair', 'Pair']], () => sel()?.presses[i]?.button, (v) => edit((d) => { devAcc(id, 'presses', i)(d)[0][i] && (d.devices.find((x) => x.id === id).presses[i].button = v); }, sec, true))),
      mk('td', {}, btn('✕', () => edit((d) => { d.devices.find((x) => x.id === id).presses.splice(i, 1); }, sec, true), { title: 'Remove this press', cls: 'tiny danger' }))));
  });
  return mk('div', {}, mk('div', { class: 'sub' }, 'Button presses during the run'),
    dev.presses.length ? mk('table', {}, body) : mk('div', { class: 'note' }, 'No scheduled presses. Ticks appear under the time bar.'),
    btn('+ Add press', () => edit((d) => { d.devices.find((x) => x.id === id).presses.push({ at_s: rnd(S.frame?.t_s ?? 0, 1), button: 'trigger' }); }, sec, true), { title: 'Schedule a button press at the current time' }));
}

/* ---------- Light panel ---------- */
zone($('#z-light'), () => cur().lighting.sources.length, (root) => {
  const W = () => cur().display.width_m;
  root.append(
    slide('Room light', (d) => [d.lighting, 'ambient'], { min: 0, max: 3, step: 0.05, fmt: (v) => (v < 0.05 ? 'dark room' : v >= 1 ? `${rnd(v, 2)} (daylight)` : rnd(v, 2)), sec: 'light', hint: 'From a dark room (0) to bright daylight (1 and above).' }),
    mk('div', { class: 'note' }, 'Bright light washes out the markers that are far from the tracker. This is a rough, qualitative model; the effect shows up in Evaluate and Export, not in the live views.'),
    mk('div', { class: 'sub' }, 'Lamps and windows'));
  cur().lighting.sources.forEach((_, i) => {
    const P = (k) => (d) => [d.lighting.sources[i].position_m, k], sec = 'light';
    root.append(mk('fieldset', {}, mk('legend', {}, `Lamp or window ${i + 1}`),
      slide('Brightness', (d) => [d.lighting.sources[i], 'intensity'], { min: 0, max: 20, step: 0.5, sec, hint: '1 = about as bright as a marker seen from 2 m; a sunny window is far higher.' }),
      mk('div', { class: 'grid2' }, num('Across', P(0), { step: 0.1, unit: 'm', sec }), num('Down', P(1), { step: 0.1, unit: 'm', sec }), num('Z', P(2), { step: 0.1, unit: 'm', sec, title: 'Negative = in front of the display' })),
      btn('Remove', () => edit((d) => { d.lighting.sources.splice(i, 1); }, sec, true), { cls: 'danger' })));
  });
  root.append(btn('+ Add lamp/window', () => edit((d) => { d.lighting.sources.push({ position_m: [rnd(W() + 0.6), 0, -0.3], intensity: 1 }); }, 'light', true), { title: 'Adds a light beside the display; drag it in the room view' }));
});

/* ---------- Radio panel ---------- */
const linksNow = () => (S.frame?.links || []).filter((l) => l.best);
zone($('#z-radio'), () => 1, (root) => {
  const chips = mk('div', { class: 'chips' });
  for (const [n, v] of Object.entries(S.presets.environments)) {
    const b = btn(ENV_INFO[n] || n, () => edit((d) => { d.radio.path_loss_exponent = v; }, 'radio', true), { title: `${ENV_INFO[n] || n}: signal fades with distance exponent ${v}` });
    sync(() => b.setAttribute('aria-pressed', cur().radio.path_loss_exponent === v));
    chips.append(b);
  }
  const R = (k) => (d) => [d.radio, k];
  root.append(mk('div', { class: 'sub' }, 'What is the room like?'), chips,
    text(() => { const r = S.derived.radio.range_m; return `Range in open air about ${r >= 100 ? Math.round(r) : rnd(r, 1)} m (a model, see below).`; }, 'div'),
    mk('div', { id: 'z-chart' }), mk('div', { id: 'z-radiofields', class: 'detailed-only' }), mk('div', { id: 'z-links', class: 'detailed-only' }),
    mk('div', { class: 'note' }, 'This is a model with datasheet-based defaults for Bluetooth radios, not measured Odyssey data.'));
  zone($('#z-radiofields', root), () => cur().radio.walls.length, (r) => {
    r.append(mk('fieldset', {}, mk('legend', {}, 'Radio settings'),
      num('Transmit power', R('tx_power_dbm'), { step: 1, unit: 'dBm', sec: 'radio', title: 'How strongly the tracker’s radio transmits' }),
      num('Receiver sensitivity', R('sensitivity_dbm'), { step: 1, unit: 'dBm', sec: 'radio', title: 'Weakest signal the dongle can still hear (more negative = better)' }),
      num('Antenna and hand loss', R('implementation_loss_db'), { step: 1, min: 0, max: 40, unit: 'dB', sec: 'radio', title: 'Loss the datasheet leaves out: small PCB antennas, the enclosure, a hand around the tracker' }),
      num('Random fading', R('fading_db'), { step: 0.5, min: 0, unit: 'dB', sec: 'radio', title: 'How much the signal wobbles from reflections' }),
      num('Path-loss exponent', R('path_loss_exponent'), { step: 0.1, min: 1, max: 6, sec: 'radio', title: '2 = open space, 3 = furnished room, 3.8 = cluttered' })),
      mk('div', { class: 'sub' }, 'Walls'));
    cur().radio.walls.forEach((_, i) => {
      const W = (k, j) => (d) => [d.radio.walls[i][k], j];
      r.append(mk('fieldset', {}, mk('legend', {}, `Wall ${i + 1}`),
        mk('div', { class: 'grid2' }, num('Start across', W('a', 0), { step: 0.1, unit: 'm', sec: 'radio' }), num('Start in front', (d) => [{ get z() { return -d.radio.walls[i].a[1]; }, set z(v) { d.radio.walls[i].a[1] = -v; } }, 'z'], { step: 0.1, unit: 'm', sec: 'radio' }),
          num('End across', W('b', 0), { step: 0.1, unit: 'm', sec: 'radio' }), num('End in front', (d) => [{ get z() { return -d.radio.walls[i].b[1]; }, set z(v) { d.radio.walls[i].b[1] = -v; } }, 'z'], { step: 0.1, unit: 'm', sec: 'radio' })),
        num('Signal lost through it', (d) => [d.radio.walls[i], 'loss_db'], { min: 0, step: 1, unit: 'dB', sec: 'radio', title: 'Drywall ≈ 3–6 dB, brick ≈ 10, concrete ≈ 15' }),
        btn('Remove wall', () => edit((d) => { d.radio.walls.splice(i, 1); }, 'radio', true), { cls: 'danger' })));
    });
    r.append(btn('+ Add wall', () => edit((d) => { const w = d.display.width_m; d.radio.walls.push({ a: [rnd(w / 2 - 1), -1], b: [rnd(w / 2 + 1), -1], loss_db: 6 }); }, 'radio', true), { title: 'Adds a wall; you can also use "Add wall" above the room view to draw one' }));
  });
  zone($('#z-chart', root), () => `${S.ver}|${linksNow().map((l) => l.endpoint)}|${cur().devices.map((d) => d.id + d.name)}`, buildChart);
  zone($('#z-links', root), () => linksNow().map((l) => l.endpoint).join(), buildLinkTable);
});
function logX(v, lo, hi, w) { return (Math.log10(v) - Math.log10(lo)) / (Math.log10(hi) - Math.log10(lo)) * w; }
function buildChart(root) {
  const r = S.derived.radio, W = 300, H = 190, L = 34, B = 24, Tp = 8, w = W - L - 6, h = H - B - Tp;
  const hi = r.curve[r.curve.length - 1][0], lo = r.curve[0][0], sens = r.sensitivity_dbm;
  const top = Math.ceil(Math.max(...r.curve.map((c) => c[1])) / 10) * 10, bot = Math.floor((sens - 14) / 10) * 10;
  const X = (d) => L + logX(clamp(d, lo, hi), lo, hi, w), Y = (v) => Tp + (top - clamp(v, bot, top)) / (top - bot) * h;
  const bands = [[sens + 20, top, 'strong', 'Strong'], [sens + 10, sens + 20, 'fair', 'Fair'], [sens, sens + 10, 'marginal', 'Marginal'], [bot, sens, 'lost', 'Lost']];
  const svg = mk('svg', { viewBox: `0 0 ${W} ${H}`, width: '100%', role: 'img', 'aria-label': 'Signal strength against distance', style: 'display:block;max-width:420px' });
  for (const [a, b, q, name] of bands) {
    svg.append(mk('rect', { x: L, y: Y(b), width: w, height: Math.max(0, Y(a) - Y(b)), fill: `var(--${q})`, opacity: 0.16 }, mk('title', {}, `${name}: ${rnd(a - sens, 0)} dB or more above sensitivity`.replace('or more', q === 'lost' ? 'below' : 'or more'))),
      mk('text', { x: L + w - 3, y: Y(b) + 11, 'text-anchor': 'end', fill: `var(--${q})`, style: 'fill:currentColor', class: `q-${q}` }, name));
  }
  for (let v = top; v >= bot; v -= 20) svg.append(mk('line', { class: 'gridline', x1: L, x2: L + w, y1: Y(v), y2: Y(v) }), mk('text', { x: L - 3, y: Y(v) + 4, 'text-anchor': 'end' }, v));
  for (let d = 10 ** Math.ceil(Math.log10(lo)); d <= hi; d *= 10) svg.append(mk('line', { class: 'gridline', x1: X(d), x2: X(d), y1: Tp, y2: Tp + h }), mk('text', { x: X(d), y: H - 10, 'text-anchor': 'middle' }, `${d} m`));
  svg.append(mk('polyline', { class: 'chartline', stroke: 'var(--fg)', points: r.curve.map(([d, v]) => `${X(d)},${Y(v)}`).join(' ') }, mk('title', {}, 'Expected signal strength at each distance')),
    mk('line', { x1: L, x2: L + w, y1: Y(sens), y2: Y(sens), stroke: 'var(--bad)', 'stroke-dasharray': '5 3' }, mk('title', {}, `Receiver sensitivity ${sens} dBm`)),
    mk('text', { x: L + 3, y: Y(sens) - 3 }, 'sensitivity'), mk('text', { x: 2, y: 8 }, 'dBm'));
  const dots = mk('g');
  linksNow().forEach((l) => {
    const dev = cur().devices.find((d) => d.id === l.endpoint), mark = l.endpoint.startsWith('marker-');
    const g = mk('g', {}, mk('circle', { r: mark ? 3 : 5, fill: mark ? 'var(--marker)' : colorOf(l.endpoint), stroke: 'var(--bg)' }), mk('title', {}),
      !mark && mk('text', { dx: 7, dy: -5, style: 'fill:var(--fg)' }, dev?.name || l.endpoint));
    g.dataset.ep = l.endpoint; dots.append(g);
  });
  svg.append(dots);
  sync(() => {
    for (const g of dots.children) {
      const l = linksNow().find((x) => x.endpoint === g.dataset.ep);
      if (!l) continue;
      g.setAttribute('transform', `translate(${X(l.distance_m)} ${Y(l.rssi_dbm)})`);
      g.querySelector('title').textContent = `${g.dataset.ep}: ${rnd(l.distance_m, 1)} m, ${rnd(l.rssi_dbm, 0)} dBm, ${QUALITY_TEXT[l.quality]}`;
    }
  });
  root.append(svg, mk('div', { class: 'note' }, 'Dots are the live links to the strongest dongle: coloured dots are trackers, yellow dots are display markers. Bands show how much signal is left above the dongle’s sensitivity.'));
}
function buildLinkTable(root) {
  const body = mk('tbody');
  linksNow().forEach((l) => {
    const cells = ['d', 'r', 'm', 'q', 'p'].map(() => mk('td'));
    sync(() => {
      const x = linksNow().find((v) => v.endpoint === l.endpoint);
      if (!x) return;
      cells[0].textContent = `${rnd(x.distance_m, 1)} m`; cells[1].textContent = `${rnd(x.rssi_dbm, 0)} dBm`; cells[2].textContent = `${rnd(x.margin_db, 0)} dB`;
      cells[3].textContent = QUALITY_TEXT[x.quality]; cells[3].className = `q-${x.quality}`; cells[4].textContent = `${rnd(x.loss * 100, 1)}%`;
    });
    body.append(mk('tr', {}, mk('td', {}, cur().devices.find((d) => d.id === l.endpoint)?.name || l.endpoint), ...cells));
  });
  root.append(mk('div', { class: 'sub' }, 'Links to the strongest dongle'),
    linksNow().length ? mk('table', {}, mk('thead', {}, mk('tr', {}, ['Endpoint', 'Distance', 'Signal', 'Headroom', 'Quality', 'Loss'].map((h) => mk('th', { title: h === 'Headroom' ? 'dB above the dongle’s sensitivity' : h === 'Loss' ? 'Expected share of lost packets' : h }, h)))), body)
      : mk('div', { class: 'note' }, 'No links yet. Add a dongle and a tracker.'));
}

/* ---------- Hub panel ---------- */
const simPost = (command, target = {}) => api('POST', '/api/sim', { ...target, command }).then((s) => { S.hub.sim = s; refresh(); }).catch(fail('hub'));
async function useHubDisplay() {
  try {
    setError('hub', ''); await flushNow();
    const before = structuredClone(cur().display);
    applyReply(await api('POST', '/api/hub/display', {}));
    // Keep the devices where they stood relative to the display.
    edit((d) => { const adopted = d.display; d.display = before; applyDisplay(d, adopted); }, 'hub', true);
  } catch (e) { setError('hub', e.message); }
}
zone($('#z-hub'), () => JSON.stringify([S.hub.sim && [S.hub.sim.units.map((u) => u.uuid), S.hub.sim.dongles.map((d) => d.uuid)], S.hub.devices.map((d) => d.uuid), !!S.hub.screen]), (root) => {
  root.append(text(() => (S.hub.connected ? `Hub connected${S.hub.sim ? ' (simulator running)' : ''}.` : `No hub reachable${S.hub.error ? `: ${S.hub.error}` : ''}.`), 'div'),
    mk('div', { class: 'sub' }, 'Devices the hub sees'));
  if (!S.hub.devices.length) root.append(mk('div', { class: 'note' }, 'None.'));
  S.hub.devices.forEach((hd) => {
    root.append(mk('div', {}, mk('b', {}, hd.name || hd.uuid), ` · firmware ${hd.firmware.join('.')} · ${hd.simulated ? 'simulated' : 'real'} · `,
      text(() => { const x = S.hub.devices.find((v) => v.uuid === hd.uuid)?.tracking; return x?.aim_uv ? `aim ${rnd(x.aim_uv[0])}, ${rnd(x.aim_uv[1])}` : 'not tracked'; })));
  });
  if (S.hub.screen && !S.hub.sim) root.append(btn('Use hub display size', () => edit((d) => { const disp = { ...d.display, width_m: S.hub.screen.width_m, height_m: S.hub.screen.height_m }; delete disp.imported; applyDisplay(d, disp); }, 'hub', true), { title: `Set the display to the hub's ${rnd(S.hub.screen.width_m)} × ${rnd(S.hub.screen.height_m)} m` }));
  if (!S.hub.sim) return;
  const sim = () => S.hub.sim, tail = (uuid) => uuid.slice(6);
  const unit = () => sim().units.find((u) => u.uuid === S.simUnit) || sim().units[0];
  const dongle = () => sim().dongles.find((d) => d.uuid === S.simDongle) || sim().dongles[0];
  const post = (command) => simPost(command, { unit: unit()?.uuid, dongle: dongle()?.uuid });
  const knows = mk('input', { type: 'checkbox' });
  knows.addEventListener('change', () => edit((d) => { d.tracker_knows_surface = knows.checked; }, 'hub', true));
  sync(() => { knows.checked = cur().tracker_knows_surface !== false; });
  root.append(mk('div', { class: 'sub' }, 'The display the hub’s tracker believes in'),
    mk('label', { class: 'f', title: 'Off: the hub is given only the flat fit, as if nobody had measured the display’s shape. Needs a hub started with --temp-config.' }, knows, mk('span', {}, 'The hub knows the staged display’s shape')),
    btn('Use the hub’s display', useHubDisplay, { title: 'Stage the display the hub is calibrated for, so simulated and real devices share it' }),
    text(() => (S.frame?.sim_note ? `The hub keeps its own calibration, so its tracker and the staged display disagree. Choose “Use the hub’s display”, or start the hub with --temp-config. (${S.frame.sim_note})` : ''), 'div', 'note warn'));
  const row = (get) => mk('div', {}, text(get));
  const list = mk('div', {});
  sim().units.forEach(({ uuid }) => list.append(row(() => { const u = sim().units.find((x) => x.uuid === uuid); return u ? `Tracker …${tail(uuid)} · ${u.usb ? 'USB' : u.dongle ? `Bluetooth via …${tail(u.dongle)}` : u.ble_in_range ? 'not connected' : 'out of range'}${u.bonded ? ' · bonded' : ''}${u.pairing ? ' · pairing' : ''} · battery ${u.battery_percent}%${u.charging ? ' charging' : ''} · ${u.shots} shots` : ''; })));
  sim().dongles.forEach(({ uuid }) => list.append(row(() => { const d = sim().dongles.find((x) => x.uuid === uuid); return d ? `Dongle …${tail(uuid)} · ${d.usb ? 'plugged in' : 'unplugged'}${d.pairing ? ' · pairing' : ''} · ${d.bonds} bonded` : ''; })));
  const bat = mk('input', { type: 'range', min: 0, max: 100, step: 1, 'aria-label': 'Battery percent' }), charge = mk('input', { type: 'checkbox' });
  const batOut = mk('output', {});
  bat.addEventListener('input', () => { batOut.textContent = `${bat.value}%`; });
  const send = () => post({ type: 'battery', percent: +bat.value, charging: charge.checked });
  bat.addEventListener('change', send); charge.addEventListener('change', send);
  sync(() => { const u = unit(); if (!u) return; if (document.activeElement !== bat) { bat.value = u.battery_percent; batOut.textContent = `${bat.value}%`; } if (document.activeElement !== charge) charge.checked = u.charging; });
  const tog = (on, off, cmd, get, tip) => { const b = btn(on, () => post(cmd(!get())), { title: tip }); sync(() => { b.textContent = get() ? off : on; }); return b; };
  root.append(mk('fieldset', {}, mk('legend', {}, 'Hub simulator'), list,
    mk('div', { class: 'note' }, 'Trackers and dongles bound to the hub are added, plugged in and paired for you. The controls below are for trying what happens when something changes.'),
    mk('div', { class: 'grid2' },
      choice('Tracker', sim().units.map((u) => [u.uuid, `…${tail(u.uuid)}`]), () => unit()?.uuid, (v) => { S.simUnit = v; refresh(); }, { title: 'The simulated tracker the buttons below act on' }),
      choice('Dongle', sim().dongles.map((d) => [d.uuid, `…${tail(d.uuid)}`]), () => dongle()?.uuid, (v) => { S.simDongle = v; refresh(); }, { title: 'The simulated dongle the buttons below act on' })),
    mk('div', { class: 'row' },
      tog('Plug in tracker USB', 'Unplug tracker USB', (v) => ({ type: 'unit_usb', plugged: v }), () => unit()?.usb), tog('Plug in dongle USB', 'Unplug dongle USB', (v) => ({ type: 'dongle_usb', plugged: v }), () => dongle()?.usb),
      tog('Move into range', 'Move out of range', (v) => ({ type: 'ble_range', in_range: v }), () => unit()?.ble_in_range, 'Toggle Bluetooth range'),
      btn('Start pairing', () => post({ type: 'pair', timeout_ms: 30000 })), btn('Cancel pairing', () => post({ type: 'cancel_pairing' })), btn('Clear bonds', () => post({ type: 'clear_bonds' })),
      btn('Shot', () => post({ type: 'shot' }), { title: 'Fire one shot from the simulated tracker' })),
    mk('label', { class: 'f' }, mk('span', {}, 'Battery'), bat, batOut), mk('label', { class: 'f' }, mk('span', {}, 'Charging'), charge),
    text(() => (S.frame?.sim_error ? `Simulator error: ${S.frame.sim_error}` : ''), 'div', 'note warn')));
});

/* ---------- Evaluate panel ---------- */
const mmOf = (frac) => frac * S.derived.width_m * 1000;
async function runEval() {
  const E = S.eval;
  if (!E.device) return;
  const token = ++E.token;
  E.busy = true; E.error = null; E.runs = []; E.ver++; setError('evaluate', '');
  await flushNow();
  E.stage = JSON.stringify(S.stage);
  const jobs = [[true, 'Tracker knows the display shape']];
  if (E.ab) jobs.push([false, 'Tracker ignores the display shape']);
  try {
    for (let i = 0; i < jobs.length; i++) {
      E.msg = `Running ${i + 1} of ${jobs.length}…`; E.ver++;
      const res = await api('POST', '/api/evaluate', { device: E.device, fps: 30, tracker_knows_surface: jobs[i][0] });
      if (token !== E.token) return;
      E.runs.push({ label: jobs[i][1], res });
    }
  } catch (e) { if (token === E.token) { E.error = e.message; setError('evaluate', e.message); } }
  if (token === E.token) { E.busy = false; E.ver++; markDirty(); refresh(); }
}
const runColor = (i) => (i ? 'var(--warn)' : 'var(--accent)');
zone($('#z-eval'), () => `${S.eval.ver}|${trackers().map((t) => t.id + t.name)}`, (root) => {
  const E = S.eval;
  if (!trackers().some((t) => t.id === E.device)) E.device = trackers()[0]?.id ?? null;
  const run = btn('Run through the real tracker', runEval, { off: E.busy || !E.device, title: 'Replays the whole run through the production tracking code.', cls: E.busy ? 'busy' : '' });
  const ab = mk('input', { type: 'checkbox', checked: E.ab, onchange: () => { E.ab = ab.checked; } });
  root.append(
    mk('div', { class: 'note' }, 'Checks how well the real tracker follows the simulated aim, with this display, light and hand handling.'),
    choice('Tracker', trackers().map((t) => [t.id, t.name]), () => E.device, (v) => { E.device = v; }, { title: 'Which tracker to evaluate' }),
    mk('label', { class: 'f', title: 'Run twice: once as the tracker really behaves, once with a tracker that wrongly assumes a flat display. Shows what the waviness costs.' }, ab, mk('span', {}, 'Compare with a tracker that ignores the display’s shape (A/B)')),
    mk('div', { class: 'row' }, run, E.busy && btn('Cancel', () => { E.token++; E.busy = false; E.ver++; refresh(); }, { title: 'Stop waiting for the result (it is ignored when it arrives)' })),
    E.busy && mk('div', { class: 'note', role: 'status' }, E.msg),
    !trackers().length && mk('div', { class: 'note' }, 'Add a tracker to evaluate it.'),
    ...(E.runs.length ? resultCards(E) : []),
    mk('div', { class: 'note' }, 'Availability = share of the time the tracker had a usable aim. Below 90% usually means markers were out of view or washed out by light. Pass/fail limits are engineering regression limits, not measured hardware accuracy.'));
});
function resultCards(E) {
  const stale = mk('div', { class: 'note warn' }, 'The scene changed since this run; run again.');
  sync(() => { stale.hidden = JSON.stringify(S.stage) === E.stage; });
  const cards = E.runs.map((r, i) => {
    const m = r.res.metrics, rows = [];
    const add = (k, v, tip) => v != null && rows.push(mk('dt', { title: tip }, k), mk('dd', {}, v));
    add('Usable aim', m.availability != null ? `${rnd(m.availability * 100, 1)}% of the time` : null, 'availability');
    add('Typical aim error', m.aim_rmse != null ? `${rnd(mmOf(m.aim_rmse), 1)} mm on the display` : null, `aim_rmse = ${m.aim_rmse} of the display width`);
    add('Biggest jump', m.max_jump != null ? `${rnd(mmOf(m.max_jump), 1)} mm` : null, `max_jump = ${m.max_jump} of the display width`);
    add('Position error', m.position_rmse_m != null ? `${rnd(m.position_rmse_m * 1000, 0)} mm` : null, 'RMS error of the tracker’s position');
    add('Matches with ground truth', `${m.truth_matches} of ${m.truth_samples}`, 'truth_matches / truth_samples');
    return mk('div', {}, mk('div', { class: 'row' }, mk('span', { class: `badge ${r.res.passed ? 'pass' : 'fail'}` }, r.res.passed ? 'PASS' : 'FAIL'), mk('b', { style: `color:${runColor(i)}` }, r.label)),
      mk('dl', { class: 'kv' }, rows), r.res.violations.length ? mk('ul', {}, r.res.violations.map((v) => mk('li', {}, v))) : null,
      r.res.rerun && mk('div', {}, mk('div', { class: 'note' }, 'Inspect in Rerun (a timeline viewer for this run; needs Rerun installed):'), mk('code', { class: 'cmd' }, r.res.rerun.command), (() => { const b = btn('Copy', () => copyText(r.res.rerun.command, b)); return b; })()));
  });
  const out = [mk('div', { class: 'sub' }, 'Results'), stale, ...cards];
  const [a, b] = E.runs;
  if (a && b && a.res.metrics.aim_rmse != null && b.res.metrics.aim_rmse != null)
    out.push(mk('div', { class: 'note' }, `Ignoring the display’s shape changes the typical aim error from ${rnd(mmOf(a.res.metrics.aim_rmse), 1)} mm to ${rnd(mmOf(b.res.metrics.aim_rmse), 1)} mm.`));
  out.push(errorChart(E));
  return out;
}
function errorChart(E) {
  const W = 300, H = 150, L = 34, B = 20, Tp = 8, w = W - L - 6, h = H - B - Tp;
  const series = E.runs.map((r) => r.res.series), tmax = Math.max(1e-6, ...series.flat().map((p) => p.t_s));
  const emax = Math.max(1e-6, ...series.flat().map((p) => (p.error == null ? 0 : mmOf(p.error))));
  const X = (t) => L + t / tmax * w, Y = (e) => Tp + h - e / emax * h;
  const svg = mk('svg', { viewBox: `0 0 ${W} ${H}`, width: '100%', role: 'img', 'aria-label': 'Aim error over time', style: 'display:block;max-width:420px' },
    mk('line', { class: 'axis', x1: L, x2: L + w, y1: Tp + h, y2: Tp + h }), mk('line', { class: 'axis', x1: L, x2: L, y1: Tp, y2: Tp + h }),
    mk('text', { x: L - 3, y: Tp + 8, 'text-anchor': 'end' }, `${rnd(emax, 0)}`), mk('text', { x: L - 3, y: Tp + h, 'text-anchor': 'end' }, '0'),
    mk('text', { x: 2, y: 8 }, 'mm'), mk('text', { x: L + w, y: H - 4, 'text-anchor': 'end' }, `time (s), ${rnd(tmax, 0)} s`));
  series.forEach((s, i) => {
    let seg = [];
    const flushSeg = () => { if (seg.length > 1) svg.append(mk('polyline', { class: 'chartline', stroke: runColor(i), points: seg.join(' ') })); seg = []; };
    for (const p of s) {
      if (p.error == null) { flushSeg(); svg.append(mk('line', { x1: X(p.t_s), x2: X(p.t_s), y1: Tp + h, y2: Tp + h - 5, stroke: 'var(--lost)' }, mk('title', {}, `Tracker lost at ${rnd(p.t_s, 1)} s`))); }
      else seg.push(`${X(p.t_s)},${Y(mmOf(p.error))}`);
    }
    flushSeg();
  });
  return mk('div', {}, mk('div', { class: 'sub' }, 'Aim error over time'), svg, mk('div', { class: 'note' }, 'Error in millimetres on the display. Red ticks: the tracker had no aim at that moment. The display view shows the true path (dashed) and the tracked path.'));
}

/* ---------- Export panel ---------- */
async function blenderSteps(device) {
  const B = S.blender;
  B.busy = true; B.error = null; B.ver++; refresh();
  try { B.data = await api('POST', '/api/export/blender', { device, fps: 30 }); } catch (e) { B.error = e.message; setError('export', e.message); }
  B.busy = false; B.ver++; refresh();
}
zone($('#z-export'), () => `${S.blender.ver}|${trackers().map((t) => t.id + t.name)}`, (root) => {
  const B = S.blender, ts = trackers();
  const pick = ts.some((t) => t.id === B.device) ? B.device : (B.device = ts[0]?.id);
  const file = mk('input', { type: 'file', accept: '.json,application/json', 'aria-label': 'Scene file to import', onchange: async () => {
    const f = file.files[0]; file.value = '';
    if (!f) return;
    try { setError('export', ''); applyReply(await api('POST', '/api/import/scene', await f.text())); } catch (e) { setError('export', e.message); }
  } });
  root.append(mk('div', { class: 'sub' }, 'Download the scene'),
    mk('div', { class: 'note' }, 'A scene file holds the display, markers and one tracker’s aim over time, for use in other tools.'),
    choice('Tracker', ts.map((t) => [t.id, t.name]), () => B.device, (v) => { B.device = v; }),
    ts.length ? mk('a', { href: `/api/export/scene?device=${encodeURIComponent(pick)}&fps=30`, download: '', class: 'btnlink' }, 'Download scene JSON') : mk('div', { class: 'note' }, 'Add a tracker first.'),
    mk('div', { class: 'sub' }, 'Edit in Blender'),
    mk('div', { class: 'note' }, 'Blender gives a more detailed view of the same scene (environment meshes, camera renders). Edits you make to the display and markers there can be brought back with the import below.'),
    btn('Show the Blender steps', () => blenderSteps(B.device), { off: B.busy || !ts.length, cls: B.busy ? 'busy' : '' }),
    B.data && mk('div', {}, mk('div', { class: 'note' }, `Scene written to ${B.data.scene_path}`),
      mk('ol', { class: 'steps' }, B.data.steps.map((s) => { const c = mk('code', { class: 'cmd' }, s.command), b = btn('Copy', () => copyText(s.command, b)); return mk('li', {}, s.title, c, b); }))),
    mk('div', { class: 'sub' }, 'Import display from a scene file'),
    mk('div', { class: 'note' }, 'Pick a scene file edited in Blender; its display shape replaces the generated one.'), file);
});
Object.defineProperty(S.blender, 'device', { value: null, writable: true, enumerable: false });

/* ---------- top bar ---------- */
function syncBar() {
  const f = S.frame, st = cur(), a = document.activeElement;
  const set = (el, fn) => { if (a !== el) fn(el); };
  set($('#name'), (e) => { e.value = st.name; });
  set($('#duration'), (e) => { e.value = st.duration_s; });
  $('#scrub').max = st.duration_s;
  if (f) { set($('#speed'), (e) => { e.value = String(f.speed); }); set($('#loop'), (e) => { e.checked = f.looping; }); }
  const hub = S.hub, pill = $('#hubpill');
  pill.textContent = hub.connected ? (hub.sim ? 'Hub connected · simulator' : 'Hub connected') : 'No hub — simulated devices only';
  pill.className = `pill ${hub.connected ? 'ok' : 'off'}`;
  pill.title = hub.error || (hub.connected ? 'The Odyssey hub is reachable' : '');
  for (const id of ['simple', 'detailed']) $(`#mode-${id}`).setAttribute('aria-pressed', (id === 'detailed') === S.detailed);
  const noTracker = !trackers().length;
  $('#empty-top').hidden = $('#empty-disp').hidden = !noTracker;
  const d = S.derived, depth = d.max_depth_m * 1000;
  $('#legend').hidden = !(depth > 0 && S.shade);
  $('#lg-lo').textContent = `−${rnd(depth, 0)} mm (toward you)`; $('#lg-hi').textContent = `+${rnd(depth, 0)} mm (away)`;
  $('#topcap').textContent = depth > 0 ? 'Display ripples drawn ×3 larger so they are visible.' : '';
  $('#addwall').setAttribute('aria-pressed', S.tool === 'wall');
}
function updateTicks() {
  const box = $('#ticks'), dur = cur().duration_s;
  box.replaceChildren();
  cur().devices.forEach((dev) => {
    for (const p of dev.presses || [])
      box.append(mk('i', { class: `tick ${p.button}${dev.id === S.selected ? ' sel' : ''}`, style: `left:${clamp(p.at_s / dur, 0, 1) * 100}%;background:${colorOf(dev.id)}`, title: `${dev.name}: ${p.button} at ${p.at_s} s` }));
  });
}
function bindBar() {
  const sc = $('#scrub');
  let last = 0;
  $('#play').onclick = () => run({ playing: !S.frame?.playing });
  $('#speed').onchange = (e) => run({ speed: +e.target.value });
  $('#loop').onchange = (e) => run({ looping: e.target.checked });
  sc.oninput = () => { S.scrub = true; if (performance.now() - last > 40) { last = performance.now(); run({ t_s: +sc.value }); } };
  sc.onchange = () => { S.scrub = false; run({ t_s: +sc.value }); };
  $('#name').oninput = (e) => edit((d) => { d.name = e.target.value; });
  $('#duration').oninput = (e) => { if (e.target.valueAsNumber > 0) edit((d) => { d.duration_s = e.target.valueAsNumber; }); };
  $('#save').onclick = () => {
    const a = mk('a', { href: URL.createObjectURL(new Blob([JSON.stringify(S.stage, null, 2)], { type: 'application/json' })), download: `${(S.stage.name || 'stage').replace(/[^\w.-]+/g, '_')}.json` });
    document.body.append(a); a.click(); a.remove(); setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  };
  $('#open').onclick = () => $('#openfile').click();
  $('#openfile').onchange = async (e) => {
    const f = e.target.files[0]; e.target.value = '';
    if (!f) return;
    try { applyReply(await api('PUT', '/api/stage', await f.text())); setError('top', ''); } catch (err) { setError('top', `Could not open ${f.name}: ${err.message}`); }
  };
  for (const id of ['simple', 'detailed']) $(`#mode-${id}`).onclick = () => setMode(id === 'detailed');
  $('#shade').onchange = (e) => { S.shade = e.target.checked; markDirty(); refresh(); };
  $('#addwall').onclick = () => { S.tool = S.tool === 'wall' ? null : 'wall'; $('#top').classList.toggle('tool', !!S.tool); refresh(); };
  document.addEventListener('change', settle);
  document.addEventListener('pointerdown', (e) => { if (e.target.type === 'range') S.sliding = true; });
  document.addEventListener('pointerup', () => { if (S.sliding) { S.sliding = false; refresh(); } });
  document.addEventListener('keydown', (e) => {
    if (e.code === 'Space' && !e.target.closest('input,select,textarea,button,summary,a,[tabindex]')) { e.preventDefault(); $('#play').click(); }
  });
  window.addEventListener('focus', () => api('GET', '/api/state').then((s) => { if (!S.draft && !putBusy) { S.hub = s.hub; applyReply(s); } }).catch(() => {}));
}
function setMode(detailed) {
  S.detailed = detailed; localStorage.setItem('odyssey.detailed', detailed ? '1' : '0');
  document.body.classList.toggle('detailed', detailed);
  if (!detailed) S.tool = null;
  markDirty(); refresh();
}

/* ---------- geometry ---------- */
function fit(svg, x0, x1, y0, y1, pad) {
  const w = svg.clientWidth || 600, h = svg.clientHeight || 300, [pl, pt, pr, pb] = pad;
  svg.setAttribute('viewBox', `0 0 ${w} ${h}`);
  const s = Math.max(1, Math.min((w - pl - pr) / (x1 - x0), (h - pt - pb) / (y1 - y0)));
  const ox = pl + ((w - pl - pr) - s * (x1 - x0)) / 2, oy = pt + ((h - pt - pb) - s * (y1 - y0)) / 2;
  return { s, w, h, X: (x) => ox + (x - x0) * s, Y: (y) => oy + (y - y0) * s, inv: (px, py) => [x0 + (px - ox) / s, y0 + (py - oy) / s] };
}
function niceStep(span, target = 6) {
  const raw = span / target, p = 10 ** Math.floor(Math.log10(raw)), f = raw / p;
  return (f < 1.5 ? 1 : f < 3.5 ? 2 : f < 7.5 ? 5 : 10) * p;
}
function ticks(a, b, step) {
  const out = [];
  for (let v = Math.ceil(a / step - 1e-9) * step; v <= b + 1e-9; v += step) out.push(+v.toFixed(6));
  return out;
}
function uvXY(u, v) {
  const sf = S.derived.surface;
  if (!sf) return [u * S.derived.width_m, v * S.derived.height_m];
  const C = sf.columns, R = sf.rows, fu = clamp(u, 0, 1) * (C - 1), fv = clamp(v, 0, 1) * (R - 1);
  const i = Math.min(C - 2, fu | 0), j = Math.min(R - 2, fv | 0), a = fu - i, b = fv - j, P = sf.positions_m;
  const at = (ii, jj, k) => P[jj * C + ii][k];
  const mix = (k) => (1 - b) * ((1 - a) * at(i, j, k) + a * at(i + 1, j, k)) + b * ((1 - a) * at(i, j + 1, k) + a * at(i + 1, j + 1, k));
  return [mix(0), mix(1)];
}
const liveTrackers = () => S.hub.devices.filter((d) => !d.simulated && d.tracking);

/* ---------- top-down room view ---------- */
function topBounds() {
  const st = S.stage, W = st.display.width_m, xs = [0, W], zs = [0, 3 * S.derived.max_depth_m];
  for (const dev of st.devices) {
    const [x, , z] = dev.position_m, m = dev.movement;
    xs.push(x); zs.push(z);
    if (m?.type === 'walk') { xs.push(m.to[0]); zs.push(m.to[2]); }
    if (m?.type === 'orbit') { xs.push(x - m.radius_m, x + m.radius_m); zs.push(z - m.radius_m, z + m.radius_m); }
  }
  for (const w of st.radio.walls) { xs.push(w.a[0], w.b[0]); zs.push(w.a[1], w.b[1]); }
  for (const l of st.lighting.sources) { xs.push(l.position_m[0]); zs.push(l.position_m[2]); }
  for (const l of liveTrackers()) { xs.push(l.tracking.position_m[0]); zs.push(l.tracking.position_m[2]); }
  let x0 = Math.min(...xs) - 0.5, x1 = Math.max(...xs) + 0.5, z0 = Math.min(...zs) - 0.5, z1 = Math.max(...zs) + 0.6;
  if (x1 - x0 < 3) { const c = (x0 + x1) / 2; x0 = c - 1.5; x1 = c + 1.5; }
  if (z1 - z0 < 3) z0 = z1 - 3;
  return [x0, x1, z0, z1];
}
function topPoint(ev) {
  const r = $('#top').getBoundingClientRect(), [x, v] = T.V.inv(ev.clientX - r.left, ev.clientY - r.top);
  return [x, -v];
}
function dragger(node, start, move, done) {
  node.addEventListener('pointerdown', (e) => {
    if (e.button || S.tool) return;
    e.preventDefault(); e.stopPropagation(); node.setPointerCapture(e.pointerId);
    S.drag = true;
    const st = start(), p0 = topPoint(e);
    const mv = (ev) => move(topPoint(ev), p0, st);
    const up = () => {
      node.removeEventListener('pointermove', mv); node.removeEventListener('pointerup', up); node.removeEventListener('pointercancel', up);
      S.drag = false; S.dragId = null; markDirty();
    };
    node.addEventListener('pointermove', mv); node.addEventListener('pointerup', up); node.addEventListener('pointercancel', up);
    done?.();
  });
}
const snap = (v) => Math.round(v * 100) / 100;
function buildTop() {
  const svg = $('#top');
  if (!S.stage || !svg.clientWidth) return;
  const st = cur(), W = S.stage.display.width_m, [x0, x1, z0, z1] = topBounds(), V = (T.V = fit(svg, x0, x1, -z1, -z0, [38, 12, 14, 22]));
  const layer = (cls) => svg.appendChild(mk('g', { class: cls }));
  svg.replaceChildren();
  T.gl = {}; T.links = {}; T.live = {};
  const grid = layer('grid'), step = niceStep(Math.max(x1 - x0, z1 - z0), 8);
  for (const x of ticks(x0, x1, step)) grid.append(mk('line', { class: 'gridline', x1: V.X(x), x2: V.X(x), y1: V.Y(-z1), y2: V.Y(-z0) }), mk('text', { x: V.X(x), y: V.h - 6, 'text-anchor': 'middle' }, `${rnd(x)}`));
  for (const z of ticks(z0, z1, step)) grid.append(mk('line', { class: 'gridline', x1: V.X(x0), x2: V.X(x1), y1: V.Y(-z), y2: V.Y(-z) }), mk('text', { x: 4, y: V.Y(-z) + 4 }, z <= 0 ? `${rnd(-z)} m` : ''));
  grid.append(mk('text', { x: V.w - 4, y: V.h - 6, 'text-anchor': 'end' }, 'm across →'), mk('text', { x: 4, y: 11 }, 'distance in front ↓'));
  const dl = layer('display');
  dl.append(mk('line', { class: 'displine', x1: V.X(0), x2: V.X(W), y1: V.Y(0), y2: V.Y(0) }, mk('title', {}, `Display, ${rnd(W)} m wide`)));
  const sf = S.derived.surface;
  if (sf && S.derived.max_depth_m > 0) {
    const row = Math.floor(sf.rows / 2), pts = [];
    for (let i = 0; i < sf.columns; i++) { const p = sf.positions_m[row * sf.columns + i]; pts.push(`${V.X(p[0])},${V.Y(-3 * p[2])}`); }
    dl.append(mk('polyline', { class: 'profile', points: pts.join(' ') }, mk('title', {}, 'Display depth along its middle row, drawn ×3')));
  }
  S.derived.markers.forEach((m, i) => dl.append(mk('circle', { class: 'marker', cx: V.X(m[0]), cy: V.Y(0), r: 3.5 }, mk('title', {}, `Infrared marker ${i + 1}`))));
  const interactive = S.detailed;
  const walls = layer('walls');
  st.radio.walls.forEach((w, i) => {
    const line = mk('line', { class: 'wallseg', x1: V.X(w.a[0]), y1: V.Y(-w.a[1]), x2: V.X(w.b[0]), y2: V.Y(-w.b[1]) }, mk('title', {}, `Wall: ${w.loss_db} dB signal loss`));
    const hit = mk('line', { class: 'wallhit', x1: line.getAttribute('x1'), y1: line.getAttribute('y1'), x2: line.getAttribute('x2'), y2: line.getAttribute('y2') });
    walls.append(line, hit, mk('text', { x: (V.X(w.a[0]) + V.X(w.b[0])) / 2 + 6, y: (V.Y(-w.a[1]) + V.Y(-w.b[1])) / 2 - 6 }, `${w.loss_db} dB`));
    if (!interactive) return;
    const move = (key) => (p, p0, s) => edit((d) => { const q = d.radio.walls[i]; for (const k of key) { q[k][0] = snap(s[k][0] + p[0] - p0[0]); q[k][1] = snap(s[k][1] + p[1] - p0[1]); } }, 'radio');
    dragger(hit, () => ({ a: [...w.a], b: [...w.b] }), move(['a', 'b']));
    for (const k of ['a', 'b']) {
      const hd = mk('circle', { class: 'handle', cx: V.X(w[k][0]), cy: V.Y(-w[k][1]), r: 6 }, mk('title', {}, 'Drag to move this end of the wall'));
      dragger(hd, () => ({ a: [...w.a], b: [...w.b] }), move([k])); walls.append(hd);
    }
  });
  const lights = layer('lights');
  st.lighting.sources.forEach((l, i) => {
    const g = mk('g', { transform: `translate(${V.X(l.position_m[0])} ${V.Y(-l.position_m[2])})` }, mk('circle', { class: 'light', r: 7 }), mk('text', { dx: 10, dy: 4 }, '☀ light'), mk('title', {}, `Lamp or window, brightness ${l.intensity}`));
    lights.append(g);
    if (interactive) dragger(g, () => [...l.position_m], (p, p0, s) => edit((d) => { const q = d.lighting.sources[i].position_m; q[0] = snap(s[0] + p[0] - p0[0]); q[2] = snap(s[2] + p[1] - p0[1]); }, 'light'));
  });
  T.rangeG = layer('ranges'); T.linksG = layer('links'); T.liveG = layer('lives'); T.devG = layer('devs');
  st.devices.forEach((dev) => {
    const color = colorOf(dev.id), tr = dev.kind === 'tracker';
    const g = mk('g', { class: `dev ${dev.kind}${dev.id === S.selected ? ' sel' : ''}`, tabindex: 0, role: 'button', 'aria-label': `${dev.name} (${dev.kind}). Arrow keys move it.` }, mk('title', {}, `${dev.name}: drag to move`));
    const o = { g };
    if (tr) {
      o.cone = mk('path', { fill: color, opacity: 0.18 }); o.ray = mk('line', { stroke: color, 'stroke-width': 1.5, 'stroke-dasharray': '4 3' });
      g.append(mk('circle', { class: 'ring', r: 15 }), mk('circle', { class: 'blob', r: 9, fill: color }), mk('circle', { r: 3, fill: 'var(--bg)' }), mk('circle', { class: 'flash', r: 9, stroke: color }));
      T.liveG.after(o.cone, o.ray);
    } else {
      o.range = mk('circle', { class: 'range', stroke: color }, mk('title', {}, 'Open-air radio range (a model)'));
      T.rangeG.append(o.range);
      g.append(mk('circle', { class: 'ring', r: 16 }), mk('circle', { class: 'arc', r: 14, stroke: color, opacity: 0.35 }), mk('circle', { class: 'arc', r: 9, stroke: color, opacity: 0.65 }), mk('circle', { class: 'blob', r: 4, fill: color }));
    }
    o.lbl = mk('text', { y: 25, 'text-anchor': 'middle' }, dev.name); o.prob = mk('text', { class: 'problem', y: 38, 'text-anchor': 'middle' });
    g.append(o.lbl, o.prob);
    g.addEventListener('pointerdown', () => { S.dragId = dev.id; selectDevice(dev.id); });
    g.addEventListener('keydown', (e) => {
      const dx = { ArrowLeft: -1, ArrowRight: 1 }[e.key] || 0, dz = { ArrowUp: 1, ArrowDown: -1 }[e.key] || 0;
      if (!dx && !dz) return;
      e.preventDefault();
      edit((d) => { const p = d.devices.find((x) => x.id === dev.id).position_m; p[0] = snap(p[0] + dx * 0.05); p[2] = Math.min(-0.3, snap(p[2] + dz * 0.05)); }, 'devices');
    });
    if (S.tool !== 'wall') dragger(g, () => [...dev.position_m], (p, p0, s) => edit((d) => {
      const q = d.devices.find((x) => x.id === dev.id).position_m; q[0] = snap(s[0] + p[0] - p0[0]); q[2] = Math.min(-0.3, snap(s[2] + p[1] - p0[1]));
    }, 'devices'));
    T.devG.append(g); T.gl[dev.id] = o;
  });
}
const placed = (dev, fd) => (dev.kind === 'tracker' && dev.movement?.type !== 'still' && fd && S.dragId !== dev.id ? fd.position_m : dev.position_m);
function paintTop(fr) {
  const V = T.V;
  if (!V) return;
  const st = cur(), fmap = new Map(fr?.devices.map((d) => [d.id, d])), pos = {};
  for (const dev of st.devices) {
    const o = T.gl[dev.id], fd = fmap.get(dev.id);
    if (!o) continue;
    const p = (pos[dev.id] = placed(dev, fd)), px = V.X(p[0]), py = V.Y(-p[2]);
    o.g.setAttribute('transform', `translate(${px} ${py})`);
    if (o.range) { o.range.setAttribute('cx', px); o.range.setAttribute('cy', py); o.range.setAttribute('r', S.derived.radio.range_m * V.s); continue; }
    const f = fd?.forward;
    if (f && f[2] > 0.01) {
      const t = -p[2] / f[2], dx = V.X(p[0] + f[0] * t) - px, dy = V.Y(0) - py, c = Math.cos(0.2), s = Math.sin(0.2);
      o.cone.setAttribute('d', `M${px} ${py}L${px + dx * c - dy * s} ${py + dx * s + dy * c}L${px + dx * c + dy * s} ${py - dx * s + dy * c}Z`);
      o.ray.setAttribute('x1', px); o.ray.setAttribute('y1', py); o.ray.setAttribute('x2', px + dx); o.ray.setAttribute('y2', py + dy);
      o.cone.style.display = o.ray.style.display = '';
    } else o.cone.style.display = o.ray.style.display = 'none';
    const prob = fd?.problem ? `⚠ ${fd.problem}` : '';
    if (o.prob.textContent !== prob) o.prob.textContent = prob;
  }
  const used = new Set();
  for (const l of fr?.links || []) {
    if (!l.best) continue;
    const key = `${l.dongle}|${l.endpoint}`, a = pos[l.dongle], mark = l.endpoint.startsWith('marker-');
    const b = mark ? (S.derived.markers[+l.endpoint.slice(7)] ? [S.derived.markers[+l.endpoint.slice(7)][0], 0, 0] : null) : pos[l.endpoint];
    if (!a || !b) continue;
    used.add(key);
    const o = (T.links[key] ||= { line: mk('line', { class: `link${mark ? ' marker' : ''}` }, mk('title', {})), lbl: mark ? null : mk('text', { class: 'linklbl', 'text-anchor': 'middle' }) });
    if (!o.line.isConnected) T.linksG.append(o.line, o.lbl);
    const x1 = V.X(a[0]), y1 = V.Y(-a[2]), x2 = V.X(b[0]), y2 = V.Y(-b[2]);
    for (const [k, v] of [['x1', x1], ['y1', y1], ['x2', x2], ['y2', y2]]) o.line.setAttribute(k, v);
    o.line.setAttribute('class', `link q-${l.quality}${mark ? ' marker' : ''}`);
    o.line.setAttribute('stroke-dasharray', DASH[l.quality]);
    const label = `${QUALITY_TEXT[l.quality]} · ${rnd(l.rssi_dbm, 0)} dBm · ${rnd(l.distance_m, 1)} m`;
    const ttl = o.line.firstChild;
    if (ttl.textContent !== `${l.endpoint}: ${label}`) ttl.textContent = `${l.endpoint}: ${label}`;
    if (o.lbl) { o.lbl.setAttribute('x', (x1 + x2) / 2); o.lbl.setAttribute('y', (y1 + y2) / 2 - 4); if (o.lbl.textContent !== label) o.lbl.textContent = label; }
  }
  for (const [k, o] of Object.entries(T.links)) if (!used.has(k)) { o.line.remove(); o.lbl?.remove(); delete T.links[k]; }
  paintLive(V);
}
function paintLive(V) {
  const W = S.derived.width_m, seen = new Set();
  for (const hd of liveTrackers()) {
    seen.add(hd.uuid);
    const p = hd.tracking.position_m, px = V.X(p[0]), py = V.Y(-p[2]);
    let o = T.live[hd.uuid];
    if (!o) {
      o = T.live[hd.uuid] = { g: mk('g', { class: 'live' }, mk('circle', { class: 'blob', r: 10, stroke: 'var(--fg)' }), mk('text', { y: 26, 'text-anchor': 'middle' }, `LIVE · ${hd.name || hd.uuid}`), mk('title', {}, 'A physical tracker reported by the hub')), ray: mk('line', { stroke: 'var(--fg)', 'stroke-dasharray': '2 3' }) };
      T.liveG.append(o.ray, o.g);
    }
    o.g.setAttribute('transform', `translate(${px} ${py})`);
    const u = hd.tracking.aim_uv;
    o.ray.style.display = u ? '' : 'none';
    if (u) { o.ray.setAttribute('x1', px); o.ray.setAttribute('y1', py); o.ray.setAttribute('x2', V.X(u[0] * W)); o.ray.setAttribute('y2', V.Y(0)); }
  }
  for (const [k, o] of Object.entries(T.live)) if (!seen.has(k)) { o.g.remove(); o.ray.remove(); delete T.live[k]; }
}

/* ---------- display (front) view ---------- */
function depthColor(z, zmax) {
  const t = clamp(z / zmax, -1, 1);
  return [t < 0 ? '#3d7bff' : '#ff4d4d', Math.abs(t) * 0.75];
}
function buildDisplay() {
  const svg = $('#disp');
  if (!S.derived || !svg.clientWidth) return;
  const d = S.derived, sf = d.surface, W = d.width_m, H = d.height_m;
  let x0 = 0, x1 = W, y0 = 0, y1 = H;
  if (sf) for (const [x, y] of sf.positions_m) { x0 = Math.min(x0, x); x1 = Math.max(x1, x); y0 = Math.min(y0, y); y1 = Math.max(y1, y); }
  const V = (D.V = fit(svg, x0, x1, y0, y1, [40, 24, 14, 12]));
  svg.replaceChildren();
  D.cross = {}; D.live = {};
  const pt = (p) => `${V.X(p[0])},${V.Y(p[1])}`;
  const step = niceStep(Math.max(W, H), 6);
  for (const x of ticks(x0, x1, step)) svg.append(mk('text', { x: V.X(x), y: 16, 'text-anchor': 'middle' }, `${rnd(x)}`));
  for (const y of ticks(y0, y1, step)) svg.append(mk('text', { x: 34, y: V.Y(y) + 4, 'text-anchor': 'end' }, `${rnd(y)}`));
  svg.append(mk('text', { x: 34, y: 16, 'text-anchor': 'end' }, 'm'));
  if (sf) {
    const C = sf.columns, R = sf.rows, P = sf.positions_m, zmax = Math.max(1e-9, d.max_depth_m);
    if (S.shade && d.max_depth_m > 0) {
      const cells = mk('g', { 'shape-rendering': 'crispEdges' });
      for (let j = 0; j < R - 1; j++) for (let i = 0; i < C - 1; i++) {
        const a = P[j * C + i], b = P[j * C + i + 1], c = P[(j + 1) * C + i + 1], e = P[(j + 1) * C + i], [fill, op] = depthColor((a[2] + b[2] + c[2] + e[2]) / 4, zmax);
        if (op > 0.03) cells.append(mk('polygon', { points: [a, b, c, e].map(pt).join(' '), fill, 'fill-opacity': op }));
      }
      svg.append(cells);
    }
    svg.append(mk('rect', { class: 'nominal', x: V.X(0), y: V.Y(0), width: W * V.s, height: H * V.s }, mk('title', {}, 'Where the display would be if it were flat and square')));
    const lines = mk('g');
    for (let i = 0; i < C; i += 4) lines.append(mk('polyline', { class: 'gridline', points: Array.from({ length: R }, (_, j) => pt(P[j * C + i])).join(' ') }));
    for (let j = 0; j < R; j += 4) lines.append(mk('polyline', { class: 'gridline', points: Array.from({ length: C }, (_, i) => pt(P[j * C + i])).join(' ') }));
    const edge = [...Array.from({ length: C }, (_, i) => P[i]), ...Array.from({ length: R }, (_, j) => P[j * C + C - 1]), ...Array.from({ length: C }, (_, i) => P[(R - 1) * C + C - 1 - i]), ...Array.from({ length: R }, (_, j) => P[(R - 1 - j) * C])];
    svg.append(lines, mk('polygon', { class: 'outline', points: edge.map(pt).join(' ') }));
  } else {
    const lines = mk('g');
    for (let i = 0; i <= 10; i++) lines.append(mk('line', { class: 'gridline', x1: V.X(W * i / 10), x2: V.X(W * i / 10), y1: V.Y(0), y2: V.Y(H) }));
    for (let j = 0; j <= 6; j++) lines.append(mk('line', { class: 'gridline', x1: V.X(0), x2: V.X(W), y1: V.Y(H * j / 6), y2: V.Y(H * j / 6) }));
    svg.append(lines, mk('rect', { class: 'outline', x: V.X(0), y: V.Y(0), width: W * V.s, height: H * V.s }));
  }
  d.markers.forEach((m, i) => svg.append(mk('circle', { class: 'marker', cx: V.X(m[0]), cy: V.Y(m[1]), r: 4.5 }, mk('title', {}, `Infrared marker ${i + 1}`))));
  const E = S.eval;
  if (S.detailed && E.runs.length) {
    const path = (series, key) => { const out = []; let seg = []; for (const p of series) { if (p[key]) seg.push(pt(uvXY(...p[key]))); else if (seg.length) { out.push(seg); seg = []; } } if (seg.length) out.push(seg); return out; };
    for (const seg of path(E.runs[0].res.series, 'truth_uv')) svg.append(mk('polyline', { class: 'truth', points: seg.join(' ') }, mk('title', {}, 'Where the tracker was really aimed')));
    E.runs.forEach((r, i) => { for (const seg of path(r.res.series, 'tracked_uv')) svg.append(mk('polyline', { class: 'tracked', stroke: runColor(i), points: seg.join(' ') }, mk('title', {}, `${r.label}: where the tracker thought it aimed`))); });
  }
  cur().devices.forEach((dev) => {
    if (dev.kind !== 'tracker') return;
    const color = colorOf(dev.id), trails = [0, 1, 2, 3].map((k) => svg.appendChild(mk('polyline', { fill: 'none', stroke: color, 'stroke-width': 2, 'stroke-linejoin': 'round', opacity: [0.15, 0.3, 0.5, 0.85][k] })));
    const g = svg.appendChild(mk('g', { class: 'cross', stroke: color, display: 'none' }, mk('circle', { r: 8 }), mk('line', { x1: -13, x2: 13 }), mk('line', { y1: -13, y2: 13 }), mk('text', { x: 11, y: -9, style: 'fill:var(--fg)' }, dev.name), mk('title', {}, `${dev.name}: where it aims`)));
    const hub = svg.appendChild(mk('g', { class: 'cross', stroke: color, display: 'none' }, mk('polygon', { points: '0,-8 8,0 0,8 -8,0' }), mk('text', { x: 11, y: 14, style: 'fill:var(--fg)' }, 'hub’s tracker'), mk('title', {}, 'Where the hub’s own tracker thinks this device points')));
    D.cross[dev.id] = { g, hub, trails };
  });
  D.liveG = svg.appendChild(mk('g'));
}
function pushTrail(id, fr, uv) {
  const tr = (S.trails[id] ||= []), last = tr[tr.length - 1];
  if (!uv) { tr.length = 0; return; }
  if (last && (fr.t_s < last[0] - 0.01 || fr.t_s - last[0] > 0.5)) tr.length = 0;
  tr.push([fr.t_s, uv[0], uv[1], performance.now()]);
  while (tr.length && performance.now() - tr[0][3] > 2000) tr.shift();
}
function paintDisplay(fr) {
  const V = D.V;
  if (!V || !fr) return;
  for (const fd of fr.devices) {
    const c = D.cross[fd.id];
    if (!c) continue;
    pushTrail(fd.id, fr, fd.aim_uv);
    if (fd.aim_uv) { const [x, y] = uvXY(...fd.aim_uv); c.g.setAttribute('transform', `translate(${V.X(x)} ${V.Y(y)})`); c.g.removeAttribute('display'); } else c.g.setAttribute('display', 'none');
    if (fd.hub_aim_uv) { const [x, y] = uvXY(...fd.hub_aim_uv); c.hub.setAttribute('transform', `translate(${V.X(x)} ${V.Y(y)})`); c.hub.removeAttribute('display'); } else c.hub.setAttribute('display', 'none');
    const tr = S.trails[fd.id], n = tr.length;
    c.trails.forEach((pl, k) => {
      const a = Math.max(0, Math.floor(k * n / 4) - 1), b = Math.floor((k + 1) * n / 4);
      pl.setAttribute('points', tr.slice(a, b).map((q) => { const [x, y] = uvXY(q[1], q[2]); return `${V.X(x)},${V.Y(y)}`; }).join(' '));
    });
  }
  const seen = new Set();
  for (const hd of liveTrackers()) {
    if (!hd.tracking.aim_uv) continue;
    seen.add(hd.uuid);
    const o = (D.live[hd.uuid] ||= D.liveG.appendChild(mk('g', { class: 'cross', stroke: 'var(--fg)' }, mk('circle', { r: 9, 'stroke-dasharray': '3 2' }), mk('line', { x1: -14, x2: 14 }), mk('line', { y1: -14, y2: 14 }), mk('text', { x: 12, y: -10, style: 'fill:var(--fg);font-weight:700' }, `LIVE · ${hd.name || hd.uuid}`))));
    const [x, y] = uvXY(...hd.tracking.aim_uv);
    o.setAttribute('transform', `translate(${V.X(x)} ${V.Y(y)})`);
  }
  for (const [k, o] of Object.entries(D.live)) if (!seen.has(k)) { o.remove(); delete D.live[k]; }
}

/* ---------- live data ---------- */
function paintBar(fr) {
  const play = $('#play'), label = fr.playing ? '⏸ Pause' : '▶ Play';
  if (play.textContent !== label) play.textContent = label;
  if (!S.scrub) $('#scrub').value = fr.t_s;
  $('#time').textContent = `${rnd(fr.t_s, 1)} / ${rnd(fr.duration_s, 0)} s`;
  const a = document.activeElement;
  if (a !== $('#speed')) $('#speed').value = String(fr.speed);
  if (a !== $('#loop')) $('#loop').checked = fr.looping;
}
function frameLoop() {
  const now = performance.now();
  if (S.dirty && !S.drag) { S.dirty = false; buildTop(); buildDisplay(); updateTicks(); S.hubDirty = true; }
  if (S.frameRaw) { S.frame = JSON.parse(S.frameRaw); S.frameRaw = null; S.paintFrame = true; }
  if (S.paintFrame || S.drag || S.hubDirty) {
    S.paintFrame = S.hubDirty = false;
    paintTop(S.frame); paintDisplay(S.frame);
    if (S.frame) paintBar(S.frame);
  }
  const live = now - S.lastFrameAt < 2000;
  if (live !== S.conn) { S.conn = live; $('#conn').hidden = live; }
  requestAnimationFrame(frameLoop);
}
function connect() {
  const es = new EventSource('/api/events');
  es.addEventListener('frame', (e) => { S.frameRaw = e.data; S.lastFrameAt = performance.now(); });
  es.addEventListener('hub', (e) => {
    S.hub = JSON.parse(e.data); S.hubDirty = true;
    const sig = liveTrackers().map((d) => d.uuid).join();
    if (sig !== T.liveSig) { T.liveSig = sig; markDirty(); }
  });
  es.addEventListener('press', (e) => {
    const o = T.gl[JSON.parse(e.data).device];
    if (!o) return;
    clearTimeout(o.flashT); o.g.classList.remove('flashing'); void o.g.getBoundingClientRect(); o.g.classList.add('flashing');
    o.flashT = setTimeout(() => o.g.classList.remove('flashing'), 300);
  });
}
function svgTool() {
  const svg = $('#top');
  svg.addEventListener('pointerdown', (e) => {
    if (S.tool !== 'wall' || !T.V) return;
    e.preventDefault(); svg.setPointerCapture(e.pointerId);
    const a = topPoint(e).map(snap);
    let i = -1;
    S.drag = true;
    const mv = (ev) => {
      const b = topPoint(ev).map(snap);
      edit((d) => { if (i < 0) { d.radio.walls.push({ a, b, loss_db: 6 }); i = d.radio.walls.length - 1; } else d.radio.walls[i].b = b; }, 'radio');
    };
    const up = () => {
      svg.removeEventListener('pointermove', mv); svg.removeEventListener('pointerup', up);
      S.drag = false; S.tool = null; svg.classList.remove('tool'); markDirty(); refresh();
    };
    svg.addEventListener('pointermove', mv); svg.addEventListener('pointerup', up);
  });
}

async function init() {
  document.body.classList.toggle('detailed', S.detailed);
  try {
    const s = await api('GET', '/api/state');
    Object.assign(S, { stage: s.stage, derived: s.derived, presets: s.presets, hub: s.hub });
  } catch (e) { setError('top', e.message); return; }
  ensureSelection(); bindBar(); svgTool(); connect();
  new ResizeObserver(markDirty).observe($('#top')); new ResizeObserver(markDirty).observe($('#disp'));
  refresh(); updateTicks();
  setInterval(refresh, 250);
  requestAnimationFrame(frameLoop);
}
init();
