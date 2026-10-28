import { Injectable, NgZone, OnDestroy, effect, inject, signal, untracked } from '@angular/core';
import {
  type Action,
  AnimationLoop,
  DetachedLaserView,
  Emulator,
  type FullLaserParams,
  type FullLiveInfo,
  type FullSliceSettings,
  type GridInfo,
  type LaserView,
  type MeshView,
  type ModelInfo,
  type OverlayPattern,
  type Preset,
  type RemotePattern,
  RemoteHost,
  type RemoteMetric,
  type RemoteStatus,
  Scan,
  SliceRunner,
  SliceWorkerClient,
  Slicer,
  type ViewLabel,
  type ViewPoint,
  builtinPresets,
  calibrationParams,
  constants,
  defaultParams,
  defaultSliceSettings,
  downloadBytes,
  init,
  isModelFile,
  isOverlayPattern,
  openPopup,
  patternParams,
  presetFromJson,
  presetToJson,
  readFileBytes,
} from '@javerik/laserline';
import { LASERLINE_CONFIG } from './config';

/** How long a status message stays visible. */
const STATUS_MS = 5000;
/** Readouts (time, frame rate, slicer state) refresh at most this often. */
const READOUT_MS = 100;
/** Settings are written to localStorage at most this often. */
const PERSIST_MS = 300;
/** Frames a remote calibration pattern stays on screen before it counts as shown. */
const PATTERN_FRAMES = 2;
/** Largest tilt a controller may set, degrees either way (the Tilt slider's range). */
const MAX_TILT_DEG = 45;
/** Default distance of the line from the camera field's edges in `fitToCameraField`, px. */
const FIT_MARGIN_PX = 8;
/** In `per_frame` playback the metric's row rate follows the frame rate only when it moves by more than this share. */
const METRIC_FPS_HOLD = 0.01;

/** Where the controls are shown; see {@link LaserlineStore.controls}. */
export type ControlsPlacement = 'docked' | 'detached' | 'hidden';

/** Snapshot of the slicer for display; see {@link LaserlineStore.slicer}. */
export interface SlicerState {
  busy: boolean;
  /** Progress 0..1 of the running slice. */
  progress: number | undefined;
  /** Settings changed since the last slice. */
  stale: boolean;
  error: string | undefined;
  lastDurationS: number | undefined;
  model: ModelInfo | undefined;
  /** Model size in the scan frame (along line, travel, height) in mm. */
  sizeMm: [number, number, number] | undefined;
  grid: GridInfo | undefined;
}

const IDLE: SlicerState = {
  busy: false,
  progress: undefined,
  stale: false,
  error: undefined,
  lastDurationS: undefined,
  model: undefined,
  sizeMm: undefined,
  grid: undefined,
};

const NO_LIVE: FullLiveInfo = {
  view_px: [0, 0],
  center_px: [0, 0],
  offset_px: 0,
  length_px: 0,
  extent_along_px: 0,
  scan: undefined as unknown as FullLiveInfo['scan'],
};

interface LaserViewEntry {
  view: LaserView;
  afterRender?: () => void;
}

interface MeshViewEntry {
  view: MeshView;
  onLabels: (labels: ViewLabel[]) => void;
  afterRender?: () => void;
}

/**
 * State shared by the laserline components: the emulator, the slicer, the loaded scan
 * and the render loop driving the views. Provide it with `provideLaserline()`.
 */
@Injectable()
export class LaserlineStore implements OnDestroy {
  private readonly config = inject(LASERLINE_CONFIG);
  private readonly zone = inject(NgZone);

  /** The wasm module is loaded and the state below is valid. */
  readonly ready = signal(false);
  /** Why loading failed. */
  readonly loadError = signal<string | undefined>(undefined);

  /** Line parameters; change them with `setParams` / `patchParams`. */
  readonly params = signal<FullLaserParams>(undefined as unknown as FullLaserParams);
  /** The simulation runs. */
  readonly playing = signal(true);
  /** Simulated seconds. */
  readonly simTime = signal(0);
  /** Measured display frame rate. */
  readonly fps = signal(0);
  /** Measurements of the last rendered live view. */
  readonly live = signal<FullLiveInfo>(NO_LIVE);
  /** The scan being played, owned by the store. */
  readonly scan = signal<Scan | undefined>(undefined);
  /** Profile currently shown. */
  readonly row = signal(0);
  /** Settings of the next slice; change them with `setSliceSettings`. */
  readonly sliceSettings = signal<FullSliceSettings>(undefined as unknown as FullSliceSettings);
  /** Re-slice automatically when the settings change. */
  readonly sliceAuto = signal(true);
  /** Slicer state for display. */
  readonly slicer = signal<SlicerState>(IDLE);
  /** The slicer panel is shown. */
  readonly slicerOpen = signal(false);
  /**
   * The window the slicer is shown in (`detachSlicer()`), if not in the page. Render the
   * slicer there with `<ll-window-portal>`.
   */
  readonly slicerWindow = signal<Window | undefined>(undefined);
  /** Latest status message, cleared after a few seconds. */
  readonly status = signal<string | undefined>(undefined);
  /** The built-in presets. */
  readonly presets = signal<Preset[]>([]);
  /** Graphics API of the first view, e.g. "WebGL2". */
  readonly backend = signal<string | undefined>(undefined);
  /**
   * The live view is shown in a window of its own (`detachView()`); `<ll-laser-view>`
   * shows a placeholder meanwhile.
   */
  readonly viewDetached = signal(false);
  /** The window showing the live view (this page or the detached one) is fullscreen. */
  readonly fullscreen = signal(false);
  /**
   * Where the controls are: in the page, in a window of their own (render them there with
   * `<ll-window-portal [window]="store.controlsWindow()">`) or hidden. The host app
   * lays out the page accordingly.
   */
  readonly controls = signal<ControlsPlacement>('docked');
  /** The window the controls are shown in while `controls()` is `'detached'`. */
  readonly controlsWindow = signal<Window | undefined>(undefined);
  /** A slicing worker is configured. */
  readonly canSlice = !!this.config.createWorker;
  /** Other pages may drive the line (`LaserlineConfig.remote`). */
  readonly remoteEnabled = !!this.config.remote;
  /**
   * Another page is calibrating a camera against the line: the view shows its patterns
   * while the parameters stay as they are, and come back when it ends.
   */
  readonly calibrating = signal(false);
  /** The part of the live view (view px) a calibrated camera sees, if one was reported. */
  readonly cameraField = signal<ViewPoint[] | undefined>(undefined);
  /** Name of the camera whose field `cameraField` is. */
  readonly cameraFieldLabel = signal<string | undefined>(undefined);
  /** Draw the camera field's outline (and the limit's) over the live view. */
  readonly showCameraField = signal(true);
  /**
   * The area (view px) a controller keeps the line in (`RemoteTarget.constrain`): every
   * parameter change is adjusted to stay inside it, so controls stop at its edges.
   */
  readonly constraint = signal<ViewPoint[] | undefined>(undefined);
  /** Who set `constraint`. */
  readonly constraintLabel = signal<string | undefined>(undefined);
  /** What the last parameter change had to give up to stay inside `constraint`. */
  readonly clamped = signal<string[]>([]);
  /** A calibration pattern drawn over the live view (white, or Gray-code stripes), while one is shown. */
  readonly overlayPattern = signal<OverlayPattern | undefined>(undefined);

  private emu?: Emulator;
  private slicerCore?: Slicer;
  private runner?: SliceRunner;
  private client?: SliceWorkerClient;
  private readonly loop = new AnimationLoop((dt) => this.frame(dt));
  private readonly laserViews = new Set<LaserViewEntry>();
  private readonly meshViews = new Set<MeshViewEntry>();
  private detached?: DetachedLaserView;
  private remote?: RemoteHost;
  /** The user's parameters as a calibration line, while `calibrating`. */
  private calibrationBase?: FullLaserParams;
  private pendingPattern?: { rendered: () => void; frames: number };
  private viewPx: ViewPoint = [0, 0];
  /** Revision of the metric mapping in the status, and the mapping it was counted for (JSON). */
  private metricRev = 0;
  private metricText = '';
  /** Frame rate the metric uses as `per_frame` row rate, held while `fps` moves by under {@link METRIC_FPS_HOLD}. */
  private metricFps = 0;
  private readonly syncWindows = () => {
    const popup = this.detached;
    // A hidden window gets no animation frames: follow one being looked at, preferring
    // the live view's.
    const shown = (w: Window | undefined) => !!w && !w.closed && w.document.visibilityState === 'visible';
    const candidates = [popup?.visible ? popup.window : undefined, window, this.slicerWindow(), this.controlsWindow()];
    this.loop.setWindow(candidates.find(shown) ?? window);
    this.fullscreen.set(popup ? popup.fullscreen : !!document.fullscreenElement);
  };
  private lastReadout = 0;
  private statusTimer?: ReturnType<typeof setTimeout>;
  private persistTimer?: ReturnType<typeof setTimeout>;

  /** Resolves once `ready`; rejects if loading failed. */
  readonly loaded: Promise<void> = init(this.config.wasmUrl).then(
    () => this.start(),
    (e: unknown) => {
      this.loadError.set(`Loading laserline failed: ${message(e)}`);
      throw e;
    },
  );

  constructor() {
    this.loaded.catch(() => undefined);
    document.addEventListener('fullscreenchange', this.syncWindows);
    document.addEventListener('visibilitychange', this.syncWindows);
    // Closing the slicer also closes its window.
    effect(() => {
      if (!this.slicerOpen()) untracked(() => this.dockSlicer());
    });
  }

  ngOnDestroy(): void {
    document.removeEventListener('fullscreenchange', this.syncWindows);
    document.removeEventListener('visibilitychange', this.syncWindows);
    this.remote?.close();
    this.dockView();
    this.dockSlicer();
    this.closeControlsWindow();
    this.loop.stop();
    this.client?.dispose();
    clearTimeout(this.statusTimer);
    if (this.persistTimer) {
      clearTimeout(this.persistTimer);
      this.persist();
    }
  }

  private start(): void {
    const stored = <T>(key: string): T | undefined => {
      const prefix = this.storagePrefix();
      if (prefix === null) return undefined;
      try {
        const text = localStorage.getItem(prefix + key);
        return text === null ? undefined : (JSON.parse(text) as T);
      } catch {
        return undefined;
      }
    };
    // Through the wasm side so stored values gain fields added since.
    const params = this.tryOr(() => new Emulator(stored('params')), () => new Emulator(undefined));
    this.emu = params;
    this.slicerCore = this.tryOr(
      () => new Slicer(stored('slice_settings'), stored<boolean>('slice_auto') ?? true),
      () => new Slicer(undefined, true),
    );
    this.params.set(this.emu.params as FullLaserParams);
    this.sliceSettings.set(this.slicerCore.settings as FullSliceSettings);
    this.sliceAuto.set(this.slicerCore.auto);
    this.presets.set(builtinPresets());
    const field = stored<{ polygon: ViewPoint[]; label: string | null }>('camera_field');
    if (field?.polygon?.length) {
      this.cameraField.set(field.polygon);
      this.cameraFieldLabel.set(field.label ?? undefined);
    }
    // Applied once the view has a size (see `frame`).
    const limit = stored<{ polygon: ViewPoint[]; label: string | null }>('constraint');
    if (limit?.polygon?.length) {
      this.constraint.set(limit.polygon);
      this.constraintLabel.set(limit.label ?? undefined);
    }
    if (this.config.remote) {
      this.remote = new RemoteHost(
        {
          status: () => this.remoteStatus(),
          begin: () => this.beginCalibration(),
          show: (pattern, rendered) => this.showPattern(pattern, rendered),
          end: () => this.endCalibration(),
          setField: (polygon, label) => this.setCameraField(polygon, label),
          fitToField: (margin) => (this.cameraField() ? this.fitToCameraField(margin) : undefined),
          constrain: (polygon, label) => this.setConstraint(polygon, label),
          setTilt: (deg) => this.setTilt(deg),
        },
        this.config.remote === true ? {} : this.config.remote,
      );
    }
    if (this.config.createWorker) {
      this.client = new SliceWorkerClient(this.config.createWorker, this.config.wasmUrl);
      this.runner = new SliceRunner(this.slicerCore, this.client, (scan) => this.showScan(scan));
    }
    this.ready.set(true);
    this.zone.runOutsideAngular(() => this.loop.start());
  }

  private tryOr<T>(f: () => T, fallback: () => T): T {
    try {
      return f();
    } catch {
      return fallback();
    }
  }

  private storagePrefix(): string | null {
    return this.config.storagePrefix === undefined ? 'laserline.' : this.config.storagePrefix;
  }

  private frame(dt: number): void {
    const emu = this.emu!;
    emu.tick(dt);
    const now = performance.now();
    this.runner?.poll(now / 1000);
    let live = this.detached?.render(emu) as FullLiveInfo | undefined;
    for (const { view, afterRender } of this.laserViews) {
      live ??= view.render(emu) as FullLiveInfo;
      afterRender?.();
    }
    if (live) {
      const resized = live.view_px[0] !== this.viewPx[0] || live.view_px[1] !== this.viewPx[1];
      this.viewPx = [live.view_px[0], live.view_px[1]];
      // The limit is in view px: apply it once the view has a size, and again when it changes.
      if (resized && this.constraint()) this.setParams(this.params());
      const pending = this.pendingPattern;
      if (pending && --pending.frames <= 0) {
        this.pendingPattern = undefined;
        pending.rendered();
      }
    }
    for (const { view, onLabels, afterRender } of this.meshViews) {
      onLabels(view.render(this.slicerCore!, emu));
      afterRender?.();
    }
    if (this.row() !== emu.currentRow) this.row.set(emu.currentRow);
    if (now - this.lastReadout >= READOUT_MS) {
      this.lastReadout = now;
      this.simTime.set(emu.simTime);
      this.fps.set(emu.fps);
      if (live && !sameLive(live, this.live())) this.live.set(live);
      this.refreshSlicer();
      this.remote?.postStatus();
    }
  }

  /** Copies the slicer's state into `slicer`. */
  refreshSlicer(): void {
    const s = this.slicerCore;
    if (!s) return;
    const model = s.model();
    const next: SlicerState = {
      busy: s.isBusy,
      progress: this.runner?.progress,
      stale: s.isStale,
      error: s.error,
      lastDurationS: s.lastDurationS,
      model,
      sizeMm: model ? (Array.from(s.modelSizeMm() ?? []) as [number, number, number]) : undefined,
      grid: model ? s.gridInfo() : undefined,
    };
    if (JSON.stringify(next) !== JSON.stringify(this.slicer())) this.slicer.set(next);
  }

  // ---- views

  /**
   * Draws `view` every frame until the returned function is called, then calls
   * `afterRender` (e.g. `CanvasBridge.present`).
   */
  attachLaserView(view: LaserView, afterRender?: () => void): () => void {
    const entry = { view, afterRender };
    this.laserViews.add(entry);
    this.backend.set(view.backend);
    return () => this.laserViews.delete(entry);
  }

  /** Like `attachLaserView`; `onLabels` gets the view's labels after each frame. */
  attachMeshView(view: MeshView, onLabels: (labels: ViewLabel[]) => void, afterRender?: () => void): () => void {
    const entry = { view, onLabels, afterRender };
    this.meshViews.add(entry);
    this.backend.update((b) => b ?? view.backend);
    return () => this.meshViews.delete(entry);
  }

  // ---- live view window

  /**
   * Moves the live view into a window of its own, e.g. to show it fullscreen on another
   * screen. Call it from a user action (click, key press): browsers block pop-ups
   * otherwise. Key presses in that window are dispatched on this page's `document.body`
   * as well, so the page's shortcuts keep working there.
   */
  detachView(): void {
    if (this.detached || !this.ready()) return;
    const view = DetachedLaserView.open({
      backend: this.config.backend ?? 'auto',
      onClose: () => this.dockView(),
      onKeyDown: (e) => forwardKey(e),
      onChange: this.syncWindows,
    });
    if (!view) {
      this.setStatus('Allow pop-ups for this page to detach the live view');
      return;
    }
    this.detached = view;
    this.viewDetached.set(true);
    this.syncWindows();
    this.syncOverlay();
    view.ready.then(
      () => {
        if (this.detached === view) this.backend.set(view.backend);
      },
      (e: unknown) => {
        if (this.detached !== view) return;
        this.dockView();
        this.setStatus(`Detaching the live view failed: ${message(e)}`);
      },
    );
  }

  /** Puts the live view back into the page and closes its window. */
  dockView(): void {
    const view = this.detached;
    if (!view) return;
    this.detached = undefined;
    view.close();
    this.viewDetached.set(false);
    this.syncWindows();
  }

  /** Detaches or docks the live view (call it from a user action). */
  toggleViewDetached(): void {
    if (this.detached) this.dockView();
    else this.detachView();
  }

  /**
   * Toggles fullscreen of the window showing the live view (the detached one, or this
   * page). Call it from a user action.
   */
  toggleFullscreen(): void {
    if (this.detached) this.detached.toggleFullscreen();
    else if (document.fullscreenElement) void document.exitFullscreen();
    else void document.documentElement.requestFullscreen().catch(() => undefined);
  }

  // ---- slicer window

  /**
   * Opens the slicer in a window of its own (call it from a user action, like
   * `detachView()`); render it there with `<ll-window-portal [window]="store.slicerWindow()">`.
   * Closing that window closes the slicer.
   */
  detachSlicer(): void {
    if (this.slicerWindow() || !this.ready()) return;
    const win = openPopup({ name: 'laserline-slicer', title: 'Object Slicer', features: 'popup,width=1200,height=820' });
    if (!win) {
      this.setStatus('Allow pop-ups for this page to open the slicer in its own window');
      return;
    }
    win.addEventListener('pagehide', () => {
      if (this.slicerWindow() !== win) return;
      this.slicerWindow.set(undefined);
      this.slicerOpen.set(false);
      this.syncWindows();
    });
    win.addEventListener('keydown', forwardKey);
    win.document.addEventListener('visibilitychange', this.syncWindows);
    this.slicerWindow.set(win);
    this.slicerOpen.set(true);
    this.syncWindows();
  }

  /** Closes the slicer's window; the slicer shows in the page again if still open. */
  dockSlicer(): void {
    const win = this.slicerWindow();
    if (!win) return;
    this.slicerWindow.set(undefined);
    win.close();
    this.syncWindows();
  }

  /** Moves the slicer between this page and a window of its own. */
  toggleSlicerDetached(): void {
    if (this.slicerWindow()) this.dockSlicer();
    else this.detachSlicer();
  }

  /** Opens or closes the slicer. */
  toggleSlicer(): void {
    this.slicerOpen.update((open) => !open);
  }

  // ---- controls placement

  private controlsBeforeHide: Exclude<ControlsPlacement, 'hidden'> = 'docked';

  /**
   * Shows the controls in a window of their own (call it from a user action, like
   * `detachView()`). Closing that window docks them again.
   */
  detachControls(): void {
    if (this.controlsWindow() || !this.ready()) return;
    const win = openPopup({ name: 'laserline-controls', title: 'Laser Controls', features: 'popup,width=420,height=800' });
    if (!win) {
      this.setStatus('Allow pop-ups for this page to open the controls in their own window');
      return;
    }
    win.addEventListener('pagehide', () => {
      if (this.controlsWindow() !== win) return;
      this.controlsWindow.set(undefined);
      this.controls.set('docked');
      this.syncWindows();
    });
    win.addEventListener('keydown', forwardKey);
    win.document.addEventListener('visibilitychange', this.syncWindows);
    this.controlsWindow.set(win);
    this.controls.set('detached');
    this.syncWindows();
  }

  /** Shows the controls in the page, closing their window. */
  dockControls(): void {
    this.closeControlsWindow();
    this.controls.set('docked');
  }

  /** Detaches docked controls; docks detached or hidden ones. */
  toggleControlsDetached(): void {
    if (this.controls() === 'docked') this.detachControls();
    else this.dockControls();
  }

  /** Hides the controls, or shows them where they were before (call it from a user action). */
  toggleControlsHidden(): void {
    const placement = this.controls();
    if (placement !== 'hidden') {
      this.controlsBeforeHide = placement;
      this.closeControlsWindow();
      this.controls.set('hidden');
    } else if (this.controlsBeforeHide === 'detached') {
      this.controls.set('docked');
      this.detachControls();
    } else {
      this.controls.set('docked');
    }
  }

  private closeControlsWindow(): void {
    const win = this.controlsWindow();
    if (!win) return;
    this.controlsWindow.set(undefined);
    win.close();
    this.syncWindows();
  }

  // ---- shortcuts

  /**
   * Runs a shortcut action (see `actionForKey` in `@javerik/laserline`). Returns false
   * if it did nothing, e.g. `'exitFullscreen'` while not fullscreen or before the
   * module is loaded, so the key's default can go ahead. Call it from the key event:
   * window actions open pop-ups.
   */
  dispatch(action: Action): boolean {
    if (!this.ready()) return false;
    switch (action) {
      case 'togglePlay':
        this.togglePlaying();
        break;
      case 'resetTime':
        this.resetTime();
        break;
      case 'stepBack':
        this.stepScan(-1);
        break;
      case 'stepForward':
        this.stepScan(1);
        break;
      case 'toggleFullscreen':
        this.toggleFullscreen();
        break;
      case 'exitFullscreen':
        if (!this.fullscreen()) return false;
        this.toggleFullscreen();
        break;
      case 'toggleControlsDetached':
        this.toggleControlsDetached();
        break;
      case 'toggleViewDetached':
        this.toggleViewDetached();
        break;
      case 'toggleControlsHidden':
        this.toggleControlsHidden();
        break;
      case 'toggleSlicer':
        this.toggleSlicer();
        break;
    }
    return true;
  }

  // ---- 3D view camera

  /** Rotates the 3D view by a pointer drag in CSS pixels. */
  orbit(dx: number, dy: number): void {
    this.slicerCore?.orbit(dx, dy);
  }

  /** Pans the 3D view by a pointer drag in CSS pixels. */
  pan(dx: number, dy: number): void {
    this.slicerCore?.pan(dx, dy);
  }

  /** Zooms the 3D view: `scroll` in pixels (positive = closer), `pinch` factor (> 1 = closer). */
  zoom(scroll: number, pinch = 1): void {
    this.slicerCore?.zoom(scroll, pinch);
  }

  /** Reframes the 3D view on the next render. */
  resetCamera(): void {
    this.slicerCore?.resetCamera();
  }

  // ---- parameters and playback

  /** Replaces all line parameters (kept inside `constraint`, if any) and persists them. */
  setParams(params: FullLaserParams): void {
    params = this.constrained(params);
    // A calibration owns the view; the parameters apply when it ends.
    if (!this.calibrating()) this.emu!.params = params;
    this.params.set(params);
    this.schedulePersist();
  }

  /** Replaces some sections of the line parameters. */
  patchParams(patch: Partial<FullLaserParams>): void {
    this.setParams({ ...this.params(), ...patch });
  }

  /** Pauses or resumes the simulation. */
  togglePlaying(): void {
    this.setPlaying(!this.playing());
  }

  /** Pauses (false) or resumes (true) the simulation. */
  setPlaying(playing: boolean): void {
    if (!this.calibrating()) this.emu!.playing = playing;
    this.playing.set(playing);
  }

  /** Rewinds the clock and the scan playback. */
  resetTime(): void {
    this.emu!.resetTime();
    this.syncPlayback();
  }

  /** Shows the profile `delta` rows away from the current one. */
  stepScan(delta: number): void {
    this.emu!.stepScan(delta);
    this.syncPlayback();
  }

  /** Jumps to profile `row`. */
  seekRow(row: number): void {
    this.emu!.seekRow(row);
    this.syncPlayback();
  }

  private syncPlayback(): void {
    this.row.set(this.emu!.currentRow);
    this.simTime.set(this.emu!.simTime);
  }

  /** Applies built-in preset `index` and rewinds the clock. */
  applyPreset(index: number): void {
    const preset = this.presets()[index];
    if (!preset) return;
    this.setParams(preset.params as FullLaserParams);
    this.emu!.simTime = 0;
    this.setStatus(`Applied “${preset.name}”`);
  }

  /** Default parameters (a fresh object each call). */
  defaults(): FullLaserParams {
    return defaultParams() as FullLaserParams;
  }

  /** Resets all line parameters to their defaults. */
  resetParams(): void {
    this.setParams(defaultParams() as FullLaserParams);
    this.setStatus('All parameters reset');
  }

  /** Downloads the parameters as `laser_preset.json`. */
  savePreset(): void {
    downloadBytes(presetToJson(this.params()), 'laser_preset.json', 'application/json');
    this.setStatus('Saved laser_preset.json');
  }

  /** Loads parameters from a preset file; errors go to `status`. */
  async loadPreset(file: File): Promise<void> {
    try {
      this.setParams(presetFromJson(await file.text()) as FullLaserParams);
      this.setStatus(`Loaded ${file.name}`);
    } catch (e) {
      this.setStatus(`Load failed: ${message(e)}`);
    }
  }

  // ---- scans and models

  /** Opens a model (into the slicer, see `MODEL_EXTENSIONS`) or a `.lscan` scan. */
  async openFile(file: File): Promise<void> {
    if (isModelFile(file.name)) await this.importModel(file);
    else if (file.name.toLowerCase().endsWith('.lscan')) await this.loadScan(file);
    else this.setStatus(`${file.name}: expected a 3D model (STL, 3MF, OBJ, PLY, glTF, STEP) or an .lscan file`);
  }

  /**
   * Loads a 3D model into the slicer (parsed in the slicing worker) and opens it; errors
   * go to `status`. A unit the file declares becomes the slice settings' unit.
   */
  async importModel(file: File): Promise<void> {
    if (!this.runner) {
      this.setStatus('Import failed: no slicing worker configured');
      return;
    }
    this.setStatus(`Importing ${file.name}…`);
    try {
      const { triangles, unit } = await this.runner.loadModel(await readFileBytes(file), file.name);
      this.sliceSettings.set(this.slicerCore!.settings as FullSliceSettings);
      this.schedulePersist();
      this.slicerOpen.set(true);
      this.setStatus(`Imported ${file.name} (${triangles} triangles${unit ? `, unit ${unit.toLowerCase()} from the file` : ''})`);
    } catch (e) {
      // Superseded by a newer import, which reports for itself.
      if (message(e) === constants().cancelled) return;
      this.setStatus(`Import failed: ${file.name}: ${message(e)}`);
    }
    this.refreshSlicer();
  }

  /** Plays a `.lscan` file without its model; its settings are shown in the slicer. */
  async loadScan(file: File): Promise<void> {
    try {
      const scan = Scan.fromBytes(await readFileBytes(file));
      this.runner?.cancel();
      this.slicerCore!.adoptScan(scan);
      this.sliceSettings.set(this.slicerCore!.settings as FullSliceSettings);
      this.emu!.seekRow(0);
      this.showScan(scan);
      this.setStatus(`Loaded ${file.name}`);
    } catch (e) {
      this.setStatus(`Load failed: ${file.name}: ${message(e)}`);
    }
    this.refreshSlicer();
  }

  /** Downloads the scan being played as an `.lscan` file. */
  saveScan(): void {
    const scan = this.scan();
    if (!scan) return;
    try {
      downloadBytes(scan.toBytes(), scan.fileName());
      this.setStatus(`Saved ${scan.fileName()}`);
    } catch (e) {
      this.setStatus(`Save failed: ${message(e)}`);
    }
  }

  private showScan(scan: Scan): void {
    this.emu!.setScan(scan);
    this.scan.set(scan);
    this.row.set(this.emu!.currentRow);
  }

  /** Replaces the slice settings and persists them. */
  setSliceSettings(settings: FullSliceSettings): void {
    this.slicerCore!.settings = settings;
    this.sliceSettings.set(settings);
    this.schedulePersist();
    this.refreshSlicer();
  }

  /** Resets the slice settings to their defaults. */
  resetSliceSettings(): void {
    this.setSliceSettings(defaultSliceSettings() as FullSliceSettings);
  }

  /** Turns automatic re-slicing on or off. */
  setSliceAuto(auto: boolean): void {
    this.slicerCore!.auto = auto;
    this.sliceAuto.set(auto);
    this.schedulePersist();
  }

  /** Slices with the current settings now. */
  sliceNow(): void {
    this.runner?.sliceNow();
    this.refreshSlicer();
  }

  // ---- camera calibration (remote)

  /**
   * Moves the line into the middle of the camera field and shrinks its length and motion
   * where needed, so it stays `margin` px inside the field everywhere. Returns whether it
   * fits; deformation and scan heights are not changed and may still be too large.
   */
  fitToCameraField(margin = FIT_MARGIN_PX): boolean {
    const field = this.cameraField();
    const [w, h] = this.viewPx;
    if (!field || !this.emu || w <= 0 || h <= 0) return false;
    const fit = this.emu.fitInto(this.params(), field, w, h, margin);
    this.setParams(fit.params as FullLaserParams);
    this.setStatus(fit.fits ? 'Line fitted into the camera field' : 'The deformation or scan is too high for the camera field');
    return fit.fits;
  }

  /** Shows (or with `undefined`, removes) the camera field outline and keeps it. */
  setCameraField(polygon: ViewPoint[] | null | undefined, label?: string | null): void {
    this.cameraField.set(polygon?.length ? polygon : undefined);
    this.cameraFieldLabel.set(polygon?.length ? (label ?? undefined) : undefined);
    this.syncOverlay();
    const prefix = this.storagePrefix();
    if (prefix === null) return;
    try {
      if (polygon?.length) localStorage.setItem(prefix + 'camera_field', JSON.stringify({ polygon, label: label ?? null }));
      else localStorage.removeItem(prefix + 'camera_field');
    } catch {
      // Not kept; the controller can send it again.
    }
  }

  /**
   * Keeps the line inside the convex `polygon` (view px) from now on, or with `null` lets it
   * go; keeps the limit across reloads. Returns what the current parameters had to give up.
   */
  setConstraint(polygon: ViewPoint[] | null | undefined, label?: string | null): string[] {
    const area = polygon?.length ? polygon : undefined;
    this.constraint.set(area);
    this.constraintLabel.set(area ? (label ?? undefined) : undefined);
    this.clamped.set([]);
    this.syncOverlay();
    const prefix = this.storagePrefix();
    if (prefix !== null) {
      try {
        if (area) localStorage.setItem(prefix + 'constraint', JSON.stringify({ polygon: area, label: label ?? null }));
        else localStorage.removeItem(prefix + 'constraint');
      } catch {
        // Not kept; the controller can send it again.
      }
    }
    if (!area) {
      this.setStatus('The line is free again');
      return [];
    }
    if (this.ready()) this.setParams(this.params());
    return this.clamped();
  }

  /**
   * Sets the line's tilt (degrees on top of its orientation, clamped to ±45) and returns the
   * tilt kept, which a limit may have changed. During a calibration it applies when that ends.
   */
  setTilt(deg: number): number {
    const tilt = Math.max(-MAX_TILT_DEG, Math.min(MAX_TILT_DEG, Number.isFinite(deg) ? deg : 0));
    const p = this.params();
    this.setParams({ ...p, line: { ...p.line, tilt_deg: tilt } });
    this.setStatus(`Tilt set to ${tilt.toFixed(2)}°`);
    return this.params().line.tilt_deg;
  }

  /** `params` adjusted to keep the line inside `constraint`; as they are without one or before the view has a size. */
  private constrained(params: FullLaserParams): FullLaserParams {
    const area = this.constraint();
    const [w, h] = this.viewPx;
    if (!area || !this.emu || w <= 0 || h <= 0) return params;
    const c = this.emu.constrainInto(params, area, w, h);
    this.clamped.set(c.clamped);
    if (!c.clamped.length) return params;
    const who = this.constraintLabel() ? ` (${this.constraintLabel()})` : '';
    this.setStatus(c.fits ? `Kept inside the limit${who}: ${c.clamped.join(', ')}` : `The line is too wide for the limit${who}`);
    return c.params as FullLaserParams;
  }

  /** Shows or hides the camera field outline. */
  setShowCameraField(show: boolean): void {
    this.showCameraField.set(show);
    this.syncOverlay();
  }

  private remoteStatus(): Omit<RemoteStatus, 'session'> {
    const [w, h] = this.viewPx;
    const ready = !!this.emu && w > 0 && h > 0;
    return {
      viewPx: [w, h],
      envelope: ready ? this.emu!.envelope(this.params(), w, h) : null,
      field: this.cameraField() ?? null,
      constraint: this.constraint() ?? null,
      tilt_deg: this.params().line.tilt_deg,
      metric: ready ? this.metric(w, h) : null,
    };
  }

  /**
   * The metric mapping of the user's line (never the emulator's: during a calibration its
   * parameters are the pattern's) in a `w`×`h` view, with `rev` bumped whenever it changed.
   */
  private metric(w: number, h: number): RemoteMetric | null {
    const params = this.params();
    const fps = this.fps();
    if (params.scan.rate === 'PerFrame' && Math.abs(fps - this.metricFps) > METRIC_FPS_HOLD * this.metricFps) {
      this.metricFps = fps;
    }
    const metric = this.emu!.metric(params, w, h, this.metricFps, this.playing());
    if (!metric) return null;
    const text = JSON.stringify(metric);
    if (text !== this.metricText) {
      this.metricText = text;
      this.metricRev++;
    }
    return { ...metric, rev: this.metricRev };
  }

  private beginCalibration(): void {
    const emu = this.emu!;
    this.calibrating.set(true);
    this.calibrationBase = calibrationParams(this.params()) as FullLaserParams;
    emu.params = patternParams(this.calibrationBase, { kind: 'blank' }, 1, 1);
    emu.playing = false;
    this.syncOverlay();
  }

  private showPattern(pattern: RemotePattern, rendered: () => void): void {
    if (!this.calibrationBase) return;
    if (isOverlayPattern(pattern)) {
      // Drawn over a blank laser view.
      this.emu!.params = patternParams(this.calibrationBase, { kind: 'blank' }, 1, 1);
      this.overlayPattern.set(pattern);
    } else {
      this.emu!.params = patternParams(this.calibrationBase, pattern, this.viewPx[0], this.viewPx[1]);
      this.overlayPattern.set(undefined);
    }
    this.detached?.setPattern(this.overlayPattern());
    // A view that has not drawn yet still counts down once it does.
    this.pendingPattern = { rendered, frames: PATTERN_FRAMES };
  }

  private endCalibration(): void {
    this.pendingPattern = undefined;
    this.calibrationBase = undefined;
    this.calibrating.set(false);
    this.overlayPattern.set(undefined);
    this.detached?.setPattern(undefined);
    this.emu!.params = this.params();
    this.emu!.playing = this.playing();
    this.syncOverlay();
  }

  /** The detached view draws the outline itself; `<ll-laser-canvas>` reads the signals. */
  private syncOverlay(): void {
    const field = this.cameraField();
    const limit = this.constraint();
    const shown = this.showCameraField() && !this.calibrating();
    this.detached?.setOverlay(shown ? field : undefined, this.cameraFieldLabel());
    this.detached?.setLimit(shown ? limit : undefined, limitLabel(this.constraintLabel()));
  }

  // ---- status and persistence

  /** Shows `msg` in `status` for a few seconds. */
  setStatus(msg: string): void {
    clearTimeout(this.statusTimer);
    this.status.set(msg);
    this.statusTimer = setTimeout(() => this.status.set(undefined), STATUS_MS);
  }

  private schedulePersist(): void {
    if (this.storagePrefix() === null || this.persistTimer) return;
    this.persistTimer = setTimeout(() => {
      this.persistTimer = undefined;
      this.persist();
    }, PERSIST_MS);
  }

  private persist(): void {
    const prefix = this.storagePrefix();
    if (prefix === null || !this.ready()) return;
    try {
      localStorage.setItem(prefix + 'params', JSON.stringify(this.params()));
      localStorage.setItem(prefix + 'slice_settings', JSON.stringify(this.sliceSettings()));
      localStorage.setItem(prefix + 'slice_auto', JSON.stringify(this.sliceAuto()));
    } catch {
      // Storage unavailable (private mode, quota): settings just do not persist.
    }
  }
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Runs this page's key handlers for a key press in one of its other windows. */
function forwardKey(e: KeyboardEvent): void {
  // Typing into the window's own fields is not a shortcut.
  if ((e.target as Element | null)?.closest?.('input, select, textarea, [contenteditable]')) return;
  const { key, code, location, repeat, ctrlKey, shiftKey, altKey, metaKey } = e;
  const copy = new KeyboardEvent('keydown', {
    key, code, location, repeat, ctrlKey, shiftKey, altKey, metaKey, bubbles: true, cancelable: true,
  });
  if (!document.body.dispatchEvent(copy)) e.preventDefault();
}

function sameLive(a: FullLiveInfo, b: FullLiveInfo): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/** The label drawn on the limit's outline. */
export function limitLabel(who: string | undefined): string {
  return who ? `limit · ${who}` : 'limit';
}
