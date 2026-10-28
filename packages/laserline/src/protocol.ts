import type { ModelUnit, SliceSettings } from '../wasm/laserline.js';

/** A model file parsed in the slicing worker: what `Mesh.fromArrays` takes. */
export interface ParsedModel {
  /** Vertex positions as x, y, z triples in model units. */
  vertices: Float32Array;
  /** Triangles as triples of indices into `vertices`. */
  triangles: Uint32Array;
  /** Unit the file declares (3MF, glTF, STEP). */
  unit?: ModelUnit;
}

/** Messages to the slicing worker. */
export type ToWorker =
  | { type: 'init'; wasm?: string }
  | { type: 'parse'; id: number; bytes: Uint8Array; name: string }
  | { type: 'model'; modelId: number; vertices: Float32Array; triangles: Uint32Array }
  | { type: 'slice'; id: number; modelId: number; settings: SliceSettings; name: string };

/** Messages from the slicing worker. `fatal` errors leave the worker unusable. */
export type FromWorker =
  | { type: 'progress'; id: number; value: number }
  | { type: 'parsed'; id: number; model: ParsedModel }
  | { type: 'done'; id: number; bytes: Uint8Array; durationS: number }
  | { type: 'error'; id: number; message: string; fatal?: boolean };
