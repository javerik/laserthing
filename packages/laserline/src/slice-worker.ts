/**
 * Worker side of the slicing protocol. A worker entry script only needs:
 *
 * ```ts
 * import { serveSlicing } from '@javerik/laserline/worker';
 * serveSlicing();
 * ```
 */
import { Mesh, init, slice } from './index.js';
import type { FromWorker, ToWorker } from './protocol.js';

/** The parts of a worker's global scope used here. */
export interface WorkerScope {
  /** Sends a message to the page. */
  postMessage(message: FromWorker, transfer?: Transferable[]): void;
  /** Receives messages from the page. */
  addEventListener(type: 'message', listener: (event: MessageEvent<ToWorker>) => void): void;
}

/** Answers `SliceWorkerClient` requests in this worker. */
export function serveSlicing(scope: WorkerScope = self as unknown as WorkerScope): void {
  let ready: Promise<void> | undefined;
  let model: { id: number; mesh: Mesh } | undefined;

  scope.addEventListener('message', async ({ data: msg }) => {
    if (msg.type === 'init') {
      ready = init(msg.wasm);
      return;
    }
    await (ready ??= init());
    const id = msg.type === 'slice' || msg.type === 'parse' ? msg.id : -1;
    try {
      if (msg.type === 'parse') {
        const mesh = Mesh.parse(msg.bytes, msg.name);
        const parsed = { vertices: mesh.vertices(), triangles: mesh.triangles(), unit: mesh.unit };
        mesh.free();
        scope.postMessage({ type: 'parsed', id, model: parsed }, [parsed.vertices.buffer, parsed.triangles.buffer]);
      } else if (msg.type === 'model') {
        model?.mesh.free();
        model = { id: msg.modelId, mesh: Mesh.fromArrays(msg.vertices, msg.triangles, undefined) };
      } else if (msg.type === 'slice') {
        if (model?.id !== msg.modelId) throw new Error('model not loaded in the worker');
        const t0 = performance.now();
        const progress = (value: number) => scope.postMessage({ type: 'progress', id, value });
        const scan = slice(model.mesh, msg.settings, msg.name, progress);
        const bytes = scan.toBytes();
        scan.free();
        const durationS = (performance.now() - t0) / 1000;
        scope.postMessage({ type: 'done', id, bytes, durationS }, [bytes.buffer]);
      }
    } catch (e) {
      // A trap (a Rust panic) leaves the module in an undefined state.
      const fatal = e instanceof WebAssembly.RuntimeError;
      const message = fatal ? `internal error (${e.message})` : e instanceof Error ? e.message : String(e);
      scope.postMessage({ type: 'error', id, message, fatal });
    }
  });
}
