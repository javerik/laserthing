import { Scan, constants, type SliceRequest } from '../wasm/laserline.js';
import type { FromWorker, ParsedModel, ToWorker } from './protocol.js';

/** Outcome of a slice run by {@link SliceWorkerClient}. */
export interface SliceResult {
  /** The sliced scan; free it when no longer needed. */
  scan: Scan;
  /** Seconds the slice took in the worker. */
  durationS: number;
}

interface Pending {
  id: number;
  resolve: (msg: FromWorker) => void;
  reject: (e: Error) => void;
  onProgress?: (value: number) => void;
}

/**
 * Parses model files and runs slices in a Web Worker that calls `serveSlicing()`. One
 * request at a time; cancelling terminates the worker, a fresh one starts with the
 * next request.
 */
export class SliceWorkerClient {
  private worker?: Worker;
  private workerModel?: number;
  private model?: { id: number; mesh: ParsedModel };
  private pending?: Pending;
  /** Ids of parse requests count down, so they never meet the slicer's request ids. */
  private nextParseId = -1;

  /**
   * @param createWorker creates the worker, e.g.
   *   `() => new Worker(new URL('./slice.worker', import.meta.url), { type: 'module' })`
   * @param wasm URL of `laserline_bg.wasm` for the worker (as passed to `init`).
   */
  constructor(
    private readonly createWorker: () => Worker,
    private readonly wasm?: string | URL,
  ) {}

  /**
   * Parses a model file in the worker (see `Mesh.parse`; STEP tessellation can take a
   * while). A running request is cancelled first.
   */
  async parse(bytes: Uint8Array, name: string): Promise<ParsedModel> {
    this.cancel();
    this.ensureWorker();
    const msg = await this.request({ type: 'parse', id: this.nextParseId--, bytes, name });
    if (msg.type !== 'parsed') throw new Error(`unexpected ${msg.type} reply`);
    return msg.model;
  }

  /** Mesh of model `modelId` (`Slicer.model().id`), sent to the worker when needed. */
  setModel(modelId: number, mesh: ParsedModel): void {
    this.model = { id: modelId, mesh };
  }

  /** A request is running. */
  get busy(): boolean {
    return this.pending !== undefined;
  }

  /** Slices `request`; a running request is cancelled first. */
  async slice(request: SliceRequest, onProgress?: (value: number) => void): Promise<SliceResult> {
    this.cancel();
    this.ensureWorker();
    if (this.workerModel !== request.modelId) {
      const model = this.model;
      if (model?.id !== request.modelId) throw new Error(`model ${request.modelId} not set`);
      // Copied, not transferred: the arrays are sent again to the next worker.
      this.post({ type: 'model', modelId: model.id, vertices: model.mesh.vertices, triangles: model.mesh.triangles });
      this.workerModel = request.modelId;
    }
    const msg = await this.request(
      { type: 'slice', id: request.id, modelId: request.modelId, settings: request.settings, name: request.name },
      onProgress,
    );
    if (msg.type !== 'done') throw new Error(`unexpected ${msg.type} reply`);
    return { scan: Scan.fromBytes(msg.bytes), durationS: msg.durationS };
  }

  /** Stops the running request; its promise rejects with `Error(constants().cancelled)`. */
  cancel(): void {
    const p = this.pending;
    if (!p) return;
    this.pending = undefined;
    this.dropWorker();
    p.reject(new Error(constants().cancelled));
  }

  /** Cancels the running request and terminates the worker. */
  dispose(): void {
    this.cancel();
    this.dropWorker();
  }

  private request(msg: ToWorker & { id: number }, onProgress?: (value: number) => void): Promise<FromWorker> {
    return new Promise<FromWorker>((resolve, reject) => {
      this.pending = { id: msg.id, resolve, reject, onProgress };
      this.post(msg);
    });
  }

  private dropWorker(): void {
    this.worker?.terminate();
    this.worker = undefined;
    this.workerModel = undefined;
  }

  private ensureWorker(): Worker {
    if (this.worker) return this.worker;
    const worker = this.createWorker();
    worker.onmessage = (e: MessageEvent<FromWorker>) => this.receive(e.data);
    worker.onerror = (e) => {
      const p = this.pending;
      this.pending = undefined;
      this.dropWorker();
      p?.reject(new Error(e.message || 'slicing worker failed'));
    };
    this.worker = worker;
    const wasm = this.wasm === undefined ? undefined : new URL(this.wasm, location.href).href;
    this.post({ type: 'init', wasm });
    return worker;
  }

  private post(msg: ToWorker): void {
    this.worker!.postMessage(msg);
  }

  private receive(msg: FromWorker): void {
    const p = this.pending;
    if (!p || msg.id !== p.id) return;
    if (msg.type === 'progress') {
      p.onProgress?.(msg.value);
      return;
    }
    this.pending = undefined;
    if (msg.type === 'error') {
      if (msg.fatal) this.dropWorker();
      p.reject(new Error(msg.message));
    } else {
      p.resolve(msg);
    }
  }
}
