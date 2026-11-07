import { type Emulator, LaserView, type LiveInfo } from '../wasm/laserline.js';
import { type OverlayPattern, drawOverlayPattern } from './pattern.js';
import { CanvasBridge, openPopup } from './popup.js';
import type { RemoteScreen } from './remote.js';

/**
 * A screen of the Window Management API (`ScreenDetailed`, from `getScreenDetails()`), as far
 * as laserline uses it.
 */
export interface ScreenLike {
  /** Left edge on the virtual desktop, CSS px. */
  left: number;
  /** Top edge on the virtual desktop, CSS px. */
  top: number;
  /** Width, CSS px. */
  width: number;
  /** Height, CSS px. */
  height: number;
  /** The screen's name. */
  label?: string;
}

/**
 * Puts `doc` fullscreen, on `screen` (a `ScreenDetailed`) when given and the browser supports
 * it. Must run in response to a user action in `doc`'s window; resolves to whether it worked.
 */
export function requestFullscreenOn(doc: Document, screen?: ScreenLike): Promise<boolean> {
  const options = screen ? ({ screen } as FullscreenOptions) : undefined;
  try {
    return doc.documentElement.requestFullscreen(options).then(
      () => true,
      () => false,
    );
  } catch {
    return Promise.resolve(false);
  }
}

/**
 * The screen `wanted` names in `screens` (`getScreenDetails().screens`): a number (or a string
 * of digits) is an index, anything else a label; `undefined` if none matches.
 */
export function pickScreen<S extends ScreenLike>(screens: readonly S[], wanted: string | number | null | undefined): S | undefined {
  if (wanted === null || wanted === undefined || wanted === '') return undefined;
  const text = String(wanted).trim();
  if (/^\d+$/.test(text)) return screens[Number(text)];
  return screens.find((s) => s.label === text);
}

/**
 * The screen `win` is on: its position and size from `win.screen`, and with `screens` (the
 * Window Management API's list, when this origin may read it) the matching entry's `label`
 * and `index`. `null` when `win` is closed.
 */
export function describeScreen(win: Window, screens?: readonly ScreenLike[] | null): RemoteScreen | null {
  if (win.closed || !win.screen) return null;
  const s = win.screen as Screen & { availLeft?: number; availTop?: number; isExtended?: boolean };
  const out: RemoteScreen = { left: s.availLeft ?? 0, top: s.availTop ?? 0, width: s.width, height: s.height };
  if (typeof s.isExtended === 'boolean') out.isExtended = s.isExtended;
  if (screens?.length) {
    const x = win.screenX + (win.outerWidth || 0) / 2;
    const y = win.screenY + (win.outerHeight || 0) / 2;
    const index = screens.findIndex((d) => x >= d.left && x < d.left + d.width && y >= d.top && y < d.top + d.height);
    if (index >= 0) {
      const d = screens[index];
      Object.assign(out, { left: d.left, top: d.top, width: d.width, height: d.height, index });
      if (d.label) out.label = d.label;
    }
  }
  return out;
}

/** Options of {@link DetachedLaserView.open}. */
export interface DetachedViewOptions {
  /** `"webgpu"`, `"webgl"` or `"auto"` (default). */
  backend?: string;
  /** Title of the window. */
  title?: string;
  /** Name of the window; opening another view with the same name replaces it. */
  name?: string;
  /** `window.open` features (default: a 1280×720 popup). */
  features?: string;
  /**
   * The user closed the window or pressed its "Dock" button: the view should go back
   * into the page. Not called by {@link DetachedLaserView.close}.
   */
  onClose: () => void;
  /** Key presses in the window, e.g. to run the page's shortcuts. */
  onKeyDown?: (e: KeyboardEvent) => void;
  /** The window was shown, hidden, or entered or left fullscreen. */
  onChange?: () => void;
}

const STYLE = `
  html, body { margin: 0; height: 100%; background: #000; overflow: hidden; }
  canvas { display: block; width: 100vw; height: 100vh; }
  button {
    position: fixed; top: 8px; right: 8px; opacity: 0; transition: opacity 0.2s;
    font: 13px system-ui, sans-serif; color: #d9dce1; background: #1d2024cc;
    border: 1px solid #31353b; border-radius: 3px; padding: 2px 8px; cursor: pointer;
  }
  body:hover button, button:focus-visible { opacity: 1; }
  .message { position: fixed; left: 0; right: 0; bottom: 24px; text-align: center; font: 16px system-ui, sans-serif; color: #e06c6c; }
  svg { position: fixed; inset: 0; width: 100vw; height: 100vh; pointer-events: none; }
  svg polygon { fill: none; stroke: #e8b44a; stroke-width: 1.5; stroke-dasharray: 6 4; vector-effect: non-scaling-stroke; }
  svg text { fill: #e8b44a; font: 12px system-ui, sans-serif; }
  svg.limit polygon { stroke: #8a9099; stroke-dasharray: none; }
  svg.limit text { fill: #8a9099; }
  :root:fullscreen button { display: none; }
  :root:fullscreen canvas { cursor: none; }
`;

/**
 * A live laser view in a window of its own (a popup), e.g. to put it fullscreen on a
 * projector while the page keeps the controls.
 *
 * Frames are rendered in this window and shown through a {@link CanvasBridge}. Call
 * {@link DetachedLaserView.render} every frame, and drive the frames from
 * {@link DetachedLaserView.window}'s animation frames while it is
 * {@link DetachedLaserView.visible}: a hidden page gets none.
 */
export class DetachedLaserView {
  /** Resolves once the view can render; rejects if no WebGPU or WebGL2 is available. */
  readonly ready: Promise<void>;
  private view?: LaserView;
  private size: [number, number] = [0, 0];
  private closed = false;
  private readonly bridge: CanvasBridge;
  private overlay?: SVGSVGElement;
  private limit?: SVGSVGElement;
  private patternCanvas?: HTMLCanvasElement;

  private constructor(
    private readonly win: Window,
    private readonly canvas: HTMLCanvasElement,
    private readonly options: DetachedViewOptions,
  ) {
    [canvas.width, canvas.height] = this.measure();
    this.bridge = new CanvasBridge(canvas);
    this.ready = LaserView.createOffscreen(this.bridge.offscreen, options.backend ?? 'auto').then(
      (view) => {
        if (this.closed) view.free();
        else this.view = view;
      },
      (e: unknown) => {
        this.message(`No WebGPU or WebGL2 available: ${e instanceof Error ? e.message : String(e)}`);
        throw e;
      },
    );
  }

  /**
   * Opens the window. Browsers only allow this in response to a user action (a click,
   * a key press); returns `undefined` if the popup was blocked.
   */
  static open(options: DetachedViewOptions): DetachedLaserView | undefined {
    const win = openPopup({
      name: options.name ?? 'laserline-live',
      title: options.title ?? 'Laser Line — Live View',
      features: options.features,
    });
    if (!win) return undefined;
    const doc = win.document;
    const style = doc.createElement('style');
    style.textContent = STYLE;
    const canvas = doc.createElement('canvas');
    const dock = doc.createElement('button');
    dock.type = 'button';
    dock.textContent = 'Dock';
    dock.title = 'Put the live view back into the page (V)';
    doc.head.append(style);
    doc.body.append(canvas, dock);

    let view: DetachedLaserView;
    try {
      view = new DetachedLaserView(win, canvas, options);
    } catch (e) {
      win.close();
      throw e;
    }
    const closedByUser = () => {
      if (view.closed) return;
      view.close();
      options.onClose();
    };
    dock.addEventListener('click', closedByUser);
    win.addEventListener('pagehide', closedByUser);
    canvas.addEventListener('dblclick', () => view.toggleFullscreen());
    win.addEventListener('keydown', (e) => options.onKeyDown?.(e));
    doc.addEventListener('visibilitychange', () => options.onChange?.());
    doc.addEventListener('fullscreenchange', () => options.onChange?.());
    return view;
  }

  /** The popup window. */
  get window(): Window {
    return this.win;
  }

  /** The window is open and not hidden (minimised, covered or on another tab). */
  get visible(): boolean {
    return !this.closed && !this.win.closed && this.win.document.visibilityState === 'visible';
  }

  /** {@link DetachedLaserView.close} ran or the user closed the window. */
  get isClosed(): boolean {
    return this.closed;
  }

  /** `"WebGPU"` or `"WebGL2"` once ready. */
  get backend(): string | undefined {
    return this.view?.backend;
  }

  /** The window is fullscreen. */
  get fullscreen(): boolean {
    return !this.closed && !this.win.closed && !!this.win.document.fullscreenElement;
  }

  /**
   * Must run in response to a user action in the popup (e.g. its key presses); enters
   * fullscreen on `screen` (a `ScreenDetailed`) when given.
   */
  toggleFullscreen(screen?: ScreenLike): void {
    if (this.closed || this.win.closed) return;
    const doc = this.win.document;
    if (doc.fullscreenElement) void doc.exitFullscreen();
    else void requestFullscreenOn(doc, screen);
  }

  /** The screen the window is on, see {@link describeScreen}. */
  screen(screens?: readonly ScreenLike[] | null): RemoteScreen | null {
    return this.closed ? null : describeScreen(this.win, screens);
  }

  /** Draws the current frame of `emulator` into the window; returns its measurements. */
  render(emulator: Emulator): LiveInfo | undefined {
    if (!this.view || this.closed) return undefined;
    if (this.win.closed) {
      // Closed without a pagehide event reaching us.
      this.close();
      this.options.onClose();
      return undefined;
    }
    const [w, h] = this.measure();
    if (w !== this.size[0] || h !== this.size[1]) {
      this.size = [w, h];
      this.view.resize(w, h);
    }
    const info = this.view.render(emulator);
    this.bridge.present();
    return info;
  }

  /**
   * Outlines `polygon` (view px, e.g. the part of the view a camera sees) over the view,
   * with an optional `label`; `undefined` removes the outline.
   */
  setOverlay(polygon: [number, number][] | undefined, label?: string): void {
    if (this.closed || this.win.closed) return;
    this.overlay?.remove();
    this.overlay = this.outline(polygon, label);
  }

  /**
   * Outlines the area the line is kept in (view px, see `RemoteTarget.constrain`) with an
   * optional `label`, drawn solid and grey; `undefined` removes it.
   */
  setLimit(polygon: [number, number][] | undefined, label?: string): void {
    if (this.closed || this.win.closed) return;
    this.limit?.remove();
    this.limit = this.outline(polygon, label, 'limit');
  }

  private outline(polygon: [number, number][] | undefined, label?: string, cls?: string): SVGSVGElement | undefined {
    if (!polygon?.length) return undefined;
    const doc = this.win.document;
    const ns = 'http://www.w3.org/2000/svg';
    const svg = doc.createElementNS(ns, 'svg');
    if (cls) svg.setAttribute('class', cls);
    const [w, h] = this.measure();
    svg.setAttribute('viewBox', `0 0 ${w} ${h}`);
    svg.setAttribute('preserveAspectRatio', 'none');
    const shape = doc.createElementNS(ns, 'polygon');
    shape.setAttribute('points', polygon.map((p) => p.join(',')).join(' '));
    svg.append(shape);
    if (label) {
      const text = doc.createElementNS(ns, 'text');
      const top = polygon.reduce((a, p) => (p[1] < a[1] || (p[1] === a[1] && p[0] < a[0]) ? p : a));
      text.setAttribute('x', String(top[0] + 6));
      text.setAttribute('y', String(top[1] + 16));
      text.textContent = label;
      svg.append(text);
    }
    doc.body.append(svg);
    return svg;
  }

  /**
   * Covers the view with a calibration pattern (see `drawOverlayPattern`), drawn at the view's
   * pixel size; `undefined` removes it.
   */
  setPattern(pattern: OverlayPattern | undefined): void {
    if (this.closed || this.win.closed) return;
    if (!pattern) {
      this.patternCanvas?.remove();
      this.patternCanvas = undefined;
      return;
    }
    if (!this.patternCanvas) {
      const c = this.win.document.createElement('canvas');
      c.style.cssText = 'position:fixed;inset:0;width:100vw;height:100vh;pointer-events:none';
      this.win.document.body.append(c);
      this.patternCanvas = c;
    }
    const [w, h] = this.measure();
    const c = this.patternCanvas;
    if (c.width !== w || c.height !== h) [c.width, c.height] = [w, h];
    const ctx = c.getContext('2d');
    if (ctx) drawOverlayPattern(ctx, pattern, w, h);
  }

  /** Releases the GPU resources and closes the window. Safe to call repeatedly. */
  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.view?.free();
    this.view = undefined;
    if (!this.win.closed) this.win.close();
  }

  /** The canvas' CSS size in device pixels of the popup's screen. */
  private measure(): [number, number] {
    const dpr = this.win.devicePixelRatio || 1;
    return [
      Math.max(1, Math.round(this.canvas.clientWidth * dpr)),
      Math.max(1, Math.round(this.canvas.clientHeight * dpr)),
    ];
  }

  private message(text: string): void {
    if (this.win.closed) return;
    const el = this.win.document.createElement('div');
    el.className = 'message';
    el.textContent = text;
    this.win.document.body.append(el);
  }
}
