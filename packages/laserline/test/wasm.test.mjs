// Runs the wasm build in Node: npm run build:wasm && npm run build && npm test
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { before, test } from 'node:test';
import * as ll from '../dist/index.js';

before(async () => {
  await ll.init(await readFile(new URL('../wasm/laserline_bg.wasm', import.meta.url)));
});

/** Binary STL of the box `lo`..`hi`, with a header starting with "solid" like many CAD exports. */
function boxStl(lo, hi) {
  const c = (i) => [i & 1 ? hi[0] : lo[0], i & 2 ? hi[1] : lo[1], i & 4 ? hi[2] : lo[2]];
  const quads = [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]];
  const tris = quads.flatMap((q) => [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]);
  const buf = new ArrayBuffer(84 + tris.length * 50);
  new Uint8Array(buf).set(new TextEncoder().encode('solid but binary'));
  const v = new DataView(buf);
  v.setUint32(80, tris.length, true);
  tris.forEach((t, k) => t.forEach((i, n) => c(i).forEach((x, a) => v.setFloat32(84 + k * 50 + 12 + n * 12 + a * 4, x, true))));
  return new Uint8Array(buf);
}

test('defaults and presets', () => {
  const p = ll.defaultParams();
  assert.equal(p.line.width_px, 6);
  const presets = ll.builtinPresets();
  assert.equal(presets.length, 6);
  for (const { params } of presets) assert.deepEqual(ll.presetFromJson(ll.presetToJson(params)), params);
  assert.equal(ll.presetFromJson('{"line":{"width_px":3.5}}').line.intensity_pct, 100);
  assert.throws(() => ll.presetFromJson('{'));
});

test('slice a box and round-trip the scan file', () => {
  const mesh = ll.Mesh.parseStl(boxStl([0, 0, 0], [20, 10, 8]));
  assert.equal(mesh.triangleCount, 12);
  const settings = { ...ll.defaultSliceSettings(), lateral_res_mm: 1, travel_step_mm: 1, margin_lateral_mm: 5, margin_travel_mm: 5, occlusion: false };
  const progress = [];
  const scan = ll.slice(mesh, settings, 'box.stl', (v) => progress.push(v));
  assert.equal(scan.cols, 31);
  assert.equal(scan.rows, 21);
  assert.equal(scan.zMaxMm, 8);
  assert.equal(scan.fileName(), 'box.lscan');
  assert.equal(scan.rowHeights(10)[15], 8);
  assert.equal(progress.at(-1), 1);
  const back = ll.Scan.fromBytes(scan.toBytes());
  assert.deepEqual(back.meta(), scan.meta());
  const runs = scan.profileRuns(10);
  assert.equal(runs.length, 1);
  assert.ok(runs[0].points instanceof Float32Array);
  assert.throws(() => ll.Scan.fromBytes(new Uint8Array([1, 2, 3])), /not a valid scan file/);
});

test('slicer state machine with debounce', () => {
  const slicer = new ll.Slicer({ ...ll.defaultSliceSettings(), lateral_res_mm: 1, travel_step_mm: 1 }, true);
  const mesh = ll.Mesh.parseStl(boxStl([0, 0, 0], [10, 5, 3]));
  const req = slicer.setModel(mesh, 'box.stl', '/models/box.stl');
  assert.equal(req.modelId, slicer.model().id);
  assert.ok(slicer.isBusy);
  assert.ok(slicer.finish(req.id, 0.25));
  assert.equal(slicer.lastDurationS, 0.25);
  slicer.settings = { ...slicer.settings, yaw_deg: 30 };
  assert.ok(slicer.isStale);
  assert.equal(slicer.poll(1.0), undefined);
  const next = slicer.poll(1.5);
  assert.equal(next.settings.yaw_deg, 30);
  assert.ok('cols' in slicer.gridInfo());
});

test('emulator plays a scan', () => {
  const emu = new ll.Emulator();
  const scan = ll.slice(ll.Mesh.parseStl(boxStl([0, 0, 0], [10, 5, 3])), { lateral_res_mm: 1, travel_step_mm: 1 }, '', null);
  emu.setScan(scan);
  emu.tick(0.1);
  emu.tick(0.1);
  assert.equal(emu.currentRow, 2);
  emu.stepScan(-1);
  assert.equal(emu.currentRow, 1);
  emu.resetTime();
  assert.equal(emu.simTime, 0);
  assert.ok(emu.scan.rows > 0, 'scan handle still usable after setScan');
});

test('plot curves', () => {
  const p = ll.defaultParams();
  const curve = ll.profileCurve(p.profile, 6, -9, 9, 19);
  assert.equal(curve.length, 19);
  assert.ok(Math.abs(curve[9] - 1) < 1e-6);
  assert.ok(Math.abs(curve[6] - 0.5) < 0.01, 'FWHM 6 px: half intensity at ±3 px');
  const rgb = ll.wavelengthToRgb(650);
  assert.ok(rgb[0] > rgb[1]);
});

test('constants', () => {
  const c = ll.constants();
  assert.equal(c.cancelled, 'cancelled');
  assert.equal(c.scanExtension, 'lscan');
  assert.ok(c.maxCols > 1000 && c.maxRows > 1000);
  assert.ok(Math.abs(c.fwhmPerSigma - 2.35482) < 1e-5);
});

test('typings hide the start hook', async () => {
  const dts = await readFile(new URL('../wasm/laserline.d.ts', import.meta.url), 'utf8');
  assert.doesNotMatch(dts, /export function start\(/);
});

test('mesh geometry and grid size', () => {
  const mesh = ll.Mesh.parseStl(boxStl([0, 0, 0], [20, 10, 8]));
  assert.equal(mesh.vertices().length, mesh.vertexCount * 3);
  assert.equal(mesh.triangles().length, 36);
  const settings = { ...ll.defaultSliceSettings(), lateral_res_mm: 1, travel_step_mm: 1, margin_lateral_mm: 5, margin_travel_mm: 5 };
  const pts = ll.toScanFrame(mesh, settings);
  const lo = [0, 1, 2].map((a) => Math.min(...pts.filter((_, i) => i % 3 === a)));
  const hi = [0, 1, 2].map((a) => Math.max(...pts.filter((_, i) => i % 3 === a)));
  assert.equal(lo[1], 0);
  assert.equal(lo[2], 0);
  assert.equal(lo[0], -hi[0]);
  const grid = ll.gridSize({ lo, hi }, settings);
  const slicer = new ll.Slicer(settings, false);
  slicer.setModel(mesh, 'box.stl', 'box.stl');
  assert.deepEqual(slicer.gridInfo(), grid);
  assert.deepEqual(grid, { cols: 31, rows: 21, bytes: 31 * 21 * 5 });
  assert.match(ll.gridSize({ lo, hi }, { ...settings, lateral_res_mm: 0 }).error, /positive/);
  slicer.clearModel();
  assert.equal(slicer.model(), undefined);
});

test('scan data access', () => {
  const settings = { ...ll.defaultSliceSettings(), lateral_res_mm: 1, travel_step_mm: 1, occlusion: false };
  const scan = ll.slice(ll.Mesh.parseStl(boxStl([0, 0, 0], [10, 5, 3])), settings, 'box.stl');
  const { rows, cols } = scan;
  assert.equal(scan.heights().length, rows * cols);
  assert.equal(scan.visibility().length, rows * cols);
  assert.deepEqual([...scan.rowVisible(2)], [...scan.visibility().slice(2 * cols, 3 * cols)]);
  const r = Math.floor(rows / 2);
  const h = scan.rowHeights(r);
  const v = scan.rowVisible(r);
  for (let i = 0; i < cols; i++) assert.ok(v[i] ? scan.seen(i, r) === h[i] : Number.isNaN(scan.seen(i, r)));
  assert.ok(v.some((x) => x === 1) && Math.max(...h.filter(Number.isFinite)) === 3);
  assert.throws(() => scan.seen(cols, 0), /out of range/);
  assert.ok(Number.isFinite(scan.uCenterMm));
  // Rows past the end are clamped.
  assert.deepEqual([...scan.rowHeights(rows + 5)], [...scan.rowHeights(rows - 1)]);
});

test('slice cancellation', () => {
  const mesh = ll.Mesh.parseStl(boxStl([0, 0, 0], [10, 5, 3]));
  assert.throws(() => ll.slice(mesh, { lateral_res_mm: 0.1, travel_step_mm: 0.1 }, '', undefined, () => true), new RegExp(`^Error: ${ll.constants().cancelled}$`));
});

test('model helpers', () => {
  const p = ll.defaultParams();
  assert.equal(ll.lineAngle({ ...p.line, orientation: 'Vertical', tilt_deg: 0 }), Math.fround(Math.PI / 2));
  assert.equal(ll.extentAlong(100, 50, 0, 1), 50);
  assert.equal(ll.motionOffset({ ...p.motion, kind: 'Static' }, 1, 100), 0);
  const d = { ...p.deform, kind: 'Sine', amplitude_px: 2, wavelength_px: 10, scroll_px_s: 0 };
  assert.deepEqual([...ll.sampleDeformation(d, 0, -5, 1, 11)], [...ll.deformCurve(d, 0, -5, 5, 11)]);
  assert.equal(ll.niceStep(100), 10);
});

test('key map', () => {
  const key = (k, mods = {}) => ll.actionForKey({ key: k, ctrlKey: false, altKey: false, metaKey: false, target: null, ...mods });
  assert.equal(key('F11'), 'toggleFullscreen');
  assert.equal(key('V'), 'toggleViewDetached');
  assert.equal(key(' '), 'togglePlay');
  assert.equal(key('s', { ctrlKey: true }), undefined);
  assert.equal(key('q'), undefined);
  assert.equal(new Set(ll.SHORTCUTS.map((s) => s.action)).size, ll.SHORTCUTS.length);
  assert.match(ll.shortcutHelp(), /^F11 fullscreen · /);
});

const golden = process.env.LASERLINE_GOLDEN;
test('slices match the native build', { skip: !golden && 'set LASERLINE_GOLDEN to a directory of golden .lscan files' }, async () => {
  const cases = JSON.parse(await readFile(`${golden}/cases.json`, 'utf8'));
  for (const { stl, name, settings, file } of cases) {
    const mesh = ll.Mesh.parseStl(await readFile(stl));
    const scan = ll.slice(mesh, settings, name, null);
    assert.deepEqual(Buffer.from(scan.toBytes()), await readFile(`${golden}/${file}`), file);
  }
});

test('calibration helpers', () => {
  const p = ll.defaultParams();
  const base = ll.calibrationParams(p);
  assert.equal(base.profile.kind, 'Hard');
  assert.equal(base.motion.kind, 'Static');
  const line = ll.patternParams(base, { kind: 'line', orientation: 'Vertical', position_pct: 25 }, 1000, 500);
  assert.deepEqual(line.line.position_pct, [25, 50]);
  assert.equal(ll.patternParams(base, { kind: 'blank' }, 1000, 500).line.intensity_pct, 0);
  const band = ll.patternParams(base, { kind: 'line', orientation: 'Vertical', position_pct: 50, width_pct: 20 }, 1000, 500);
  assert.equal(band.line.width_px, 200);

  const emu = new ll.Emulator(undefined);
  const e = emu.envelope(base, 1000, 500);
  assert.deepEqual(e.center_px, [500, 250]);
  assert.equal(e.corners.length, 4);
  const fit = emu.fitInto(p, [[400, 100], [700, 100], [700, 300], [400, 300]], 1000, 500, 10);
  assert.equal(fit.fits, true);
  assert.ok(fit.params.line.length < p.line.length);
  const kept = emu.constrainInto(p, [[400, 100], [700, 100], [700, 300], [400, 300]], 1000, 500);
  assert.equal(kept.fits, true);
  assert.ok(kept.clamped.includes('length'));
  assert.deepEqual(emu.constrainInto(p, [[0, 0], [1000, 0], [1000, 500], [0, 500]], 1000, 500).clamped, []);
  const deformed = { ...p, deform: { ...p.deform, kind: 'Step', height_px: -100 } };
  const heights = emu.fitHeights(deformed, [-30, 20], 1000, 500);
  assert.equal(heights.fits, true);
  assert.ok(heights.clamped.includes('deformation'));
  assert.equal(heights.params.line.length, deformed.line.length);
  assert.equal(emu.fitHeights(p, [5, 5], 1000, 500).fits, false);
  emu.free();
});

test('metric mapping of the scan being played', () => {
  const e = new ll.Emulator(undefined);
  assert.equal(e.metric(e.params, 1000, 500, 60, true), null, 'no scan');
  const scan = ll.slice(ll.Mesh.parseStl(boxStl([0, 0, 0], [20, 10, 8])), { lateral_res_mm: 1, travel_step_mm: 0.5, margin_lateral_mm: 5, occlusion: false }, '');
  e.setScan(scan);
  const m = e.metric(e.params, 1000, 500, 60, true);
  assert.deepEqual(m.base, { center_px: [500, 250], dir: [1, 0], normal: [-0, 1] });
  assert.ok(Math.abs(m.along_px_per_mm - 800 / 30) < 1e-4);
  assert.equal(m.height_px_per_mm, -m.along_px_per_mm);
  assert.deepEqual([m.dv_mm, m.rows, m.travel.mode, m.travel.mm_per_s], [0.5, scan.rows, 'per_frame', 30]);
  assert.deepEqual(m.object_mm, { min: [-10, 0, 8], max: [10, 10, 8] });
  assert.deepEqual([m.model_mirrored, m.disturbances], [false, []]);
  const off = { ...e.params, scan: { ...e.params.scan, enabled: false } };
  assert.equal(e.metric(off, 1000, 500, 60, true), null, 'scan disabled');
  e.clearScan();
  assert.equal(e.metric(e.params, 1000, 500, 60, true), null);
});
