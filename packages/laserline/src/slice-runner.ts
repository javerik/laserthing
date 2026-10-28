import { Mesh, type ModelUnit, type Scan, type SliceRequest, type Slicer } from '../wasm/laserline.js';
import type { SliceWorkerClient } from './slice-client.js';

/** A model loaded by {@link SliceRunner.loadModel}. */
export interface LoadedModel {
  /** Number of triangles. */
  triangles: number;
  /** Unit the file declares; the slicer's settings now use it. */
  unit?: ModelUnit;
}

/**
 * Connects a `Slicer` to a `SliceWorkerClient`: runs what the slicer requests,
 * cancels superseded slices and reports results back. Call `poll` every frame.
 */
export class SliceRunner {
  /** Progress (0..1) of the running slice, `undefined` when idle. */
  progress: number | undefined;
  private job: number | undefined;
  private parsing = 0;

  /** `onScan` receives each current scan; it owns it and must free it. */
  constructor(
    /** The slicer whose requests are run. */
    readonly slicer: Slicer,
    private readonly client: SliceWorkerClient,
    private readonly onScan: (scan: Scan) => void,
  ) {}

  /**
   * Parses a model file in the worker (any format of `constants().modelExtensions`) and
   * loads it. Rejects on errors, and with `Error(constants().cancelled)` when another
   * load supersedes it.
   */
  async loadModel(bytes: Uint8Array, name: string, location = name): Promise<LoadedModel> {
    // A slice started meanwhile would cancel the parse; polling waits for it instead.
    this.parsing++;
    let parsed;
    try {
      parsed = await this.client.parse(bytes, name);
    } finally {
      this.parsing--;
    }
    const mesh = Mesh.fromArrays(parsed.vertices, parsed.triangles, parsed.unit);
    const request = this.slicer.setModel(mesh, name, location);
    mesh.free();
    this.client.setModel(this.slicer.model()!.id, parsed);
    if (request) this.run(request);
    return { triangles: parsed.triangles.length / 3, unit: parsed.unit };
  }

  /** Slices with the current settings. */
  sliceNow(): void {
    const request = this.slicer.start();
    if (request) this.run(request);
  }

  /** Call every frame with a clock in seconds (e.g. `performance.now() / 1000`). */
  poll(nowS: number): void {
    if (this.parsing) return;
    if (this.job !== undefined && this.slicer.running !== this.job) this.cancel();
    const request = this.slicer.poll(nowS);
    if (request) this.run(request);
  }

  /** Stops the running slice; the slicer records it as cancelled, not as an error. */
  cancel(): void {
    this.client.cancel();
    this.job = undefined;
    this.progress = undefined;
  }

  private run(request: SliceRequest): void {
    this.client.cancel();
    this.job = request.id;
    this.progress = 0;
    const current = () => this.job === request.id;
    this.client
      .slice(request, (p) => {
        if (current()) this.progress = p;
      })
      .then(
        ({ scan, durationS }) => {
          if (this.slicer.finish(request.id, durationS)) this.onScan(scan);
          else scan.free();
        },
        (e: unknown) => this.slicer.fail(request.id, e instanceof Error ? e.message : String(e)),
      )
      .finally(() => {
        if (current()) {
          this.job = undefined;
          this.progress = undefined;
        }
      });
  }
}
