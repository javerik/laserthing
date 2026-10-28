/**
 * Laser line emulator and laser-triangulation slicer of 3D models for the browser.
 *
 * Call {@link init} once before using anything else. Then:
 * - `Emulator` + `LaserView`: the animated laser line on a canvas,
 *   {@link DetachedLaserView}: the same in a window of its own,
 * - `Slicer` + `MeshView` + {@link SliceRunner}: model import (STL, 3MF, OBJ, PLY,
 *   glTF/GLB, STEP) and slicing in a Web Worker, and the 3D preview,
 * - `Scan`, `Mesh`, presets and plot helpers,
 * - {@link actionForKey}: the keyboard shortcuts shared with the desktop app,
 * - {@link RemoteHost}: lets another page (e.g. a camera calibration) drive the line.
 */
import initWasm from '../wasm/laserline.js';
import type { InitInput, LaserParams, LiveInfo, SliceSettings } from '../wasm/laserline.js';

export * from '../wasm/laserline.js';
export { AnimationLoop, type FrameScheduler } from './loop.js';
export { DetachedLaserView, type DetachedViewOptions } from './detached-view.js';
export { CanvasBridge, openPopup, type PopupOptions } from './popup.js';
export { MODEL_ACCEPT, MODEL_EXTENSIONS, downloadBytes, isModelFile, readFileBytes } from './files.js';
export { SliceWorkerClient, type SliceResult } from './slice-client.js';
export type { FromWorker, ParsedModel, ToWorker } from './protocol.js';
export { SliceRunner, type LoadedModel } from './slice-runner.js';
export { type Action, type Shortcut, SHORTCUTS, actionForKey, shortcutHelp } from './keys.js';
export {
  type OverlayPattern,
  type RemotePattern,
  drawOverlayPattern,
  grayCode,
  isOverlayPattern,
  stripeLit,
} from './pattern.js';
export {
  REMOTE_CHANNEL,
  REMOTE_IDLE_MS,
  REMOTE_VERSION,
  RemoteHost,
  type FromLaserline,
  type RemoteHostOptions,
  type RemoteMetric,
  type RemoteStatus,
  type RemoteTarget,
  type ToLaserline,
  type ViewPoint,
} from './remote.js';

/** `T` with all fields required, recursively (generated types mark defaulted fields optional). */
export type DeepRequired<T> = T extends readonly unknown[]
  ? T
  : T extends object
    ? { [K in keyof T]-?: DeepRequired<T[K]> }
    : T;

/** Complete laser parameters, as returned by `defaultParams()` and `Emulator.params`. */
export type FullLaserParams = DeepRequired<LaserParams>;
/** Complete slice settings, as returned by `defaultSliceSettings()` and `Slicer.settings`. */
export type FullSliceSettings = DeepRequired<SliceSettings>;
/** Measurements of a rendered frame. */
export type FullLiveInfo = DeepRequired<LiveInfo>;

let ready: Promise<void> | undefined;

/** URL of the WebAssembly module inside this package (for bundlers that support it). */
export function defaultWasmUrl(): URL {
  return new URL('../wasm/laserline_bg.wasm', import.meta.url);
}

/**
 * Loads the WebAssembly module. `wasm` is where to fetch `laserline_bg.wasm` from;
 * bundlers that do not resolve `new URL(…, import.meta.url)` (e.g. Angular) should copy
 * the file to their assets and pass its URL. Repeated calls share the first load.
 */
export function init(wasm?: InitInput | Promise<InitInput>): Promise<void> {
  ready ??= initWasm({ module_or_path: wasm ?? defaultWasmUrl() }).then(() => undefined);
  return ready;
}
