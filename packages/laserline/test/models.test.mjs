// Model formats in the wasm build, and parsing + slicing through the worker protocol
// (with the worker run in-process). Needs the wasm and TypeScript builds, see wasm.test.mjs.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { before, test } from 'node:test';
import * as ll from '../dist/index.js';
import { serveSlicing } from '../dist/slice-worker.js';

const models = new URL('../../../models/', import.meta.url);
const read = async (name) => new Uint8Array(await readFile(new URL(name, models)));
const MM = { Mm: 1, Cm: 10, M: 1000, Inch: 25.4 };
const FILES = { 'l-block.stl': undefined, 'l-block.obj': undefined, 'l-block.ply': undefined, 'l-block.glb': 'M', 'l-block.3mf': 'Mm', 'l-block.step': 'Mm' };

before(async () => {
  await ll.init(await readFile(new URL('../wasm/laserline_bg.wasm', import.meta.url)));
});

/** A `Worker` whose other end is `serveSlicing` in this thread. */
class FakeWorker {
  constructor() {
    this.listeners = [];
    this.alive = true;
    serveSlicing({
      postMessage: (msg) => setTimeout(() => this.alive && this.onmessage?.({ data: structuredClone(msg) })),
      addEventListener: (_type, listener) => this.listeners.push(listener),
    });
  }
  postMessage(msg) {
    const data = structuredClone(msg);
    setTimeout(() => this.alive && this.listeners.forEach((l) => l({ data })));
  }
  terminate() {
    this.alive = false;
  }
}

test('every model format reads as the L-block', async () => {
  for (const [name, unit] of Object.entries(FILES)) {
    const mesh = ll.Mesh.parse(await read(name), name);
    assert.equal(mesh.unit, unit, name);
    const f = MM[unit ?? 'Mm'];
    const { lo, hi } = mesh.bounds();
    lo.concat(hi).forEach((c, k) => assert.ok(Math.abs(c * f - [0, 0, 0, 80, 60, 20][k]) < 1e-3, `${name}: ${lo}..${hi}`));
    mesh.free();
  }
  assert.deepEqual(ll.constants().modelExtensions, ll.MODEL_EXTENSIONS);
  assert.equal(ll.MODEL_ACCEPT, '.stl,.3mf,.obj,.ply,.gltf,.glb,.step,.stp');
  assert.ok(ll.isModelFile('Part.STP') && !ll.isModelFile('scan.lscan') && !ll.isModelFile('stl'));
  assert.throws(() => ll.Mesh.parse(new TextEncoder().encode('v 1 2 3\nf 1 2 9\n'), 'bad.obj'), /^Error: OBJ: line 2/);
});

test('meshes from arrays', () => {
  const mesh = ll.Mesh.fromArrays(new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0]), new Uint32Array([0, 1, 2]), 'Cm');
  assert.equal(mesh.triangleCount, 1);
  assert.equal(mesh.unit, 'Cm');
  assert.deepEqual([...mesh.triangles()], [0, 1, 2]);
  assert.throws(() => ll.Mesh.fromArrays(new Float32Array(3), new Uint32Array([0, 1, 2]), undefined), /out of range/);
});

test('the runner parses in the worker and adopts the unit', async () => {
  const slicer = new ll.Slicer({ ...ll.defaultSliceSettings(), unit: 'Inch', lateral_res_mm: 1, travel_step_mm: 1 }, true);
  const client = new ll.SliceWorkerClient(() => new FakeWorker());
  let onScan;
  const sliced = new Promise((resolve) => (onScan = resolve));
  const runner = new ll.SliceRunner(slicer, client, (s) => onScan(s));
  const loaded = await runner.loadModel(await read('l-block.step'), 'l-block.step');
  assert.equal(loaded.unit, 'Mm');
  assert.ok(loaded.triangles >= 24, `${loaded.triangles} triangles`);
  assert.equal(slicer.settings.unit, 'Mm');
  assert.equal(slicer.model().name, 'l-block.step');
  const scan = await sliced;
  assert.equal(scan.zMaxMm, 20);
  scan.free();

  // A new worker after a cancel gets the mesh again.
  client.cancel();
  const again = await client.slice(slicer.start());
  assert.equal(again.scan.zMaxMm, 20);
  again.scan.free();

  await assert.rejects(runner.loadModel(new TextEncoder().encode('nonsense'), 'x.ply'), /^Error: PLY: /);
  const first = runner.loadModel(await read('l-block.3mf'), 'l-block.3mf');
  const second = runner.loadModel(await read('l-block.glb'), 'l-block.glb');
  await assert.rejects(first, new RegExp(`^Error: ${ll.constants().cancelled}$`));
  assert.equal((await second).unit, 'M');
  assert.equal(slicer.settings.unit, 'M');
  await runner.loadModel(await read('l-block.ply'), 'l-block.ply');
  assert.equal(slicer.settings.unit, 'Inch', "a file without a unit gets the user's back");
  client.dispose();
});
