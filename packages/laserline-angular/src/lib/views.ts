import {
  ChangeDetectionStrategy,
  Component,
  DestroyRef,
  ElementRef,
  afterNextRender,
  afterRenderEffect,
  computed,
  inject,
  input,
  output,
  signal,
  viewChild,
} from '@angular/core';
import { CanvasBridge, LaserView, MeshView, drawOverlayPattern, type Scan, type ViewLabel, type ViewPoint } from '@javerik/laserline';
import { LASERLINE_CONFIG } from './config';
import { LaserlineStore, limitLabel } from './store';
import { THEME } from './widgets';

/** The window showing `el` (this one, or a popup the element was moved into). */
function windowOf(el: Element): Window & typeof globalThis {
  return (el.ownerDocument.defaultView ?? window) as Window & typeof globalThis;
}

/** Keeps a canvas' drawing buffer at its CSS size × device pixel ratio. */
function trackSize(canvas: HTMLCanvasElement, resize: (w: number, h: number) => void): () => void {
  const win = windowOf(canvas);
  const apply = () => {
    const dpr = win.devicePixelRatio || 1;
    const w = Math.max(1, Math.round(canvas.clientWidth * dpr));
    const h = Math.max(1, Math.round(canvas.clientHeight * dpr));
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    resize(w, h);
  };
  apply();
  const observer = new win.ResizeObserver(apply);
  observer.observe(canvas);
  return () => observer.disconnect();
}

/**
 * Binds a GPU view to the component's canvas once the store is loaded, and releases it
 * when the component is destroyed. `attach` returns the cleanups; it gets a function
 * to call after each render.
 *
 * In another window (a popup the component was moved into) the view renders into an
 * `OffscreenCanvas` of this window, and `afterRender` shows the frame on the canvas.
 */
function bindView<V extends { free(): void; resize(w: number, h: number): void }>(
  canvas: () => HTMLCanvasElement,
  create: (canvas: HTMLCanvasElement | OffscreenCanvas) => Promise<V>,
  attach: (view: V, afterRender?: () => void) => (() => void)[],
  onError: (message: string) => void,
): void {
  const store = inject(LaserlineStore);
  let cleanup: (() => void)[] = [];
  let destroyed = false;
  inject(DestroyRef).onDestroy(() => {
    destroyed = true;
    cleanup.forEach((f) => f());
  });
  afterNextRender(async () => {
    const el = canvas();
    const dpr = windowOf(el).devicePixelRatio || 1;
    el.width = Math.max(1, Math.round(el.clientWidth * dpr));
    el.height = Math.max(1, Math.round(el.clientHeight * dpr));
    try {
      await store.loaded;
      const bridge = el.ownerDocument === document ? undefined : new CanvasBridge(el);
      const view = await create(bridge?.offscreen ?? el);
      if (destroyed) return view.free();
      cleanup = [
        trackSize(el, (w, h) => view.resize(w, h)),
        ...attach(view, bridge && (() => bridge.present())),
        () => view.free(),
      ];
    } catch (e) {
      onError(`No WebGPU or WebGL2 available: ${e instanceof Error ? e.message : String(e)}`);
    }
  });
}

/**
 * The live laser line, filling the element, on a canvas of this page. Prefer
 * `<ll-laser-view>`, which can also move the view into a window of its own.
 */
@Component({
  selector: 'll-laser-canvas',
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <canvas #canvas (dblclick)="activate.emit()"></canvas>
    <canvas #pattern class="pattern" [hidden]="!store.overlayPattern()"></canvas>
    @for (f of outlines(); track f.kind) {
      <svg class="field" [class.limit]="f.kind === 'limit'" [attr.viewBox]="f.viewBox" preserveAspectRatio="none" aria-hidden="true">
        <polygon [attr.points]="f.points" />
        @if (f.label) {
          <text [attr.x]="f.labelAt[0]" [attr.y]="f.labelAt[1]">{{ f.label }}</text>
        }
      </svg>
    }
    @if (error(); as e) {
      <div class="message error">{{ e }}</div>
    }
  `,
  styles: [
    THEME,
    `
      :host { display: block; position: relative; background: #000; overflow: hidden; }
      canvas { display: block; width: 100%; height: 100%; }
      .message { position: absolute; left: 0; right: 0; bottom: 24px; text-align: center; }
      .field { position: absolute; inset: 0; width: 100%; height: 100%; pointer-events: none; }
      .pattern { position: absolute; inset: 0; width: 100%; height: 100%; pointer-events: none; }
      .pattern[hidden] { display: none; }
      .field polygon { fill: none; stroke: var(--_warn); stroke-width: 1.5; stroke-dasharray: 6 4; vector-effect: non-scaling-stroke; }
      .field text { fill: var(--_warn); font-size: 12px; }
      .field.limit polygon { stroke: var(--_weak); stroke-dasharray: none; }
      .field.limit text { fill: var(--_weak); }
    `,
  ],
})
export class LlLaserCanvas {
  private readonly canvas = viewChild.required<ElementRef<HTMLCanvasElement>>('canvas');
  /** Double click, e.g. to toggle fullscreen. */
  readonly activate = output<void>();
  protected readonly error = signal<string | undefined>(undefined);
  protected readonly store = inject(LaserlineStore);
  private readonly patternCanvas = viewChild.required<ElementRef<HTMLCanvasElement>>('pattern');
  /**
   * The camera field's outline (dashed) and the limit's (solid), in the drawing buffer's
   * pixels; hidden while calibrating.
   */
  protected readonly outlines = computed(() => {
    const [w, h] = this.store.live().view_px;
    if (!this.store.showCameraField() || this.store.calibrating() || w <= 0 || h <= 0) return [];
    const outline = (kind: 'field' | 'limit', polygon: ViewPoint[] | undefined, label: string | undefined) => {
      if (!polygon?.length) return [];
      const top = polygon.reduce((a, p) => (p[1] < a[1] || (p[1] === a[1] && p[0] < a[0]) ? p : a));
      return [{ kind, viewBox: `0 0 ${w} ${h}`, points: polygon.map((p) => p.join(',')).join(' '), label, labelAt: [top[0] + 6, top[1] + 16] }];
    };
    return [
      ...outline('field', this.store.cameraField(), this.store.cameraFieldLabel()),
      ...outline('limit', this.store.constraint(), limitLabel(this.store.constraintLabel())),
    ];
  });

  constructor() {
    const store = this.store;
    const backend = inject(LASERLINE_CONFIG).backend ?? 'auto';
    // A calibration pattern, drawn at the laser canvas' pixel size so view px match.
    afterRenderEffect({
      write: () => {
        const pattern = store.overlayPattern();
        const [w, h] = store.live().view_px;
        if (!pattern || w <= 0 || h <= 0) return;
        const c = this.patternCanvas().nativeElement;
        if (c.width !== w || c.height !== h) [c.width, c.height] = [w, h];
        const ctx = c.getContext('2d');
        if (ctx) drawOverlayPattern(ctx, pattern, w, h);
      },
    });
    bindView(
      () => this.canvas().nativeElement,
      (el) => (el instanceof OffscreenCanvas ? LaserView.createOffscreen(el, backend) : LaserView.create(el, backend)),
      (view, afterRender) => [store.attachLaserView(view, afterRender)],
      (e) => this.error.set(e),
    );
  }
}

/**
 * The live laser line, filling the element. It can be detached into a window of its own
 * (hover button, or `LaserlineStore.detachView()`) and docked back; while detached the
 * element shows a placeholder.
 */
@Component({
  selector: 'll-laser-view',
  imports: [LlLaserCanvas],
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    @if (store.viewDetached()) {
      <div class="detached">
        <span>The live view is in its own window.</span>
        <button type="button" (click)="store.dockView()">Dock here</button>
      </div>
    } @else {
      <ll-laser-canvas (activate)="activate.emit()" />
      @if (detachable()) {
        <button type="button" class="detach" title="Show the live view in a window of its own" (click)="store.detachView()">
          ⧉ Detach
        </button>
      }
    }
  `,
  styles: [
    THEME,
    `
      :host { display: block; position: relative; background: #000; overflow: hidden; }
      ll-laser-canvas { position: absolute; inset: 0; }
      .detach { position: absolute; top: 8px; right: 8px; opacity: 0; transition: opacity 0.2s; }
      :host(:hover) .detach, .detach:focus-visible { opacity: 1; }
      .detached { position: absolute; inset: 0; display: grid; place-content: center; justify-items: center; gap: 10px; color: var(--_weak); background: var(--_bg); }
    `,
  ],
})
export class LlLaserView {
  protected readonly store = inject(LaserlineStore);
  /** Show the hover button that detaches the view (the store's methods work regardless). */
  readonly detachable = input(true);
  /** Double click, e.g. to toggle fullscreen (`LaserlineStore.toggleFullscreen()`). */
  readonly activate = output<void>();
}

/** The slicer's 3D view: drag to orbit, right/shift-drag to pan, wheel to zoom, double-click to reset. */
@Component({
  selector: 'll-mesh-view',
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <canvas
      #canvas
      (pointerdown)="down($event)"
      (pointermove)="move($event)"
      (pointerup)="up($event)"
      (pointercancel)="up($event)"
      (wheel)="wheel($event)"
      (dblclick)="store.resetCamera()"
      (contextmenu)="$event.preventDefault()"
    ></canvas>
    @for (l of labels(); track l.text) {
      <span class="label" [style.left.%]="l.x * 100" [style.top.%]="l.y * 100" [style.color]="color(l)">{{ l.text }}</span>
    }
    @if (error(); as e) {
      <div class="empty error">{{ e }}</div>
    } @else if (labels().length === 0) {
      <div class="empty">Import a 3D model to start</div>
    }
    <div class="help">drag: orbit · right/shift-drag: pan · wheel: zoom · double-click: reset</div>
  `,
  styles: [
    THEME,
    `
      :host { display: block; position: relative; overflow: hidden; background: var(--ll-view-bg, #121315); touch-action: none; }
      canvas { display: block; width: 100%; height: 100%; cursor: grab; }
      canvas:active { cursor: grabbing; }
      .label { position: absolute; transform: translate(0, -100%); font-size: 13px; pointer-events: none; white-space: nowrap; }
      .empty { position: absolute; inset: 0; display: grid; place-items: center; color: var(--_weak); font-size: 18px; pointer-events: none; }
      .help { position: absolute; left: 8px; bottom: 6px; font-size: 11px; color: var(--_weak); pointer-events: none; }
    `,
  ],
})
export class LlMeshView {
  protected readonly store = inject(LaserlineStore);
  private readonly canvas = viewChild.required<ElementRef<HTMLCanvasElement>>('canvas');
  protected readonly labels = signal<ViewLabel[]>([]);
  protected readonly error = signal<string | undefined>(undefined);
  private drag?: { id: number; x: number; y: number };

  constructor() {
    const backend = inject(LASERLINE_CONFIG).backend ?? 'auto';
    bindView(
      () => this.canvas().nativeElement,
      (el) => (el instanceof OffscreenCanvas ? MeshView.createOffscreen(el, backend) : MeshView.create(el, backend)),
      (view, afterRender) => [
        this.store.attachMeshView(
          view,
          (labels) => {
            if (JSON.stringify(labels) !== JSON.stringify(this.labels())) this.labels.set(labels);
          },
          afterRender,
        ),
      ],
      (e) => this.error.set(e),
    );
  }

  protected color(l: ViewLabel): string {
    return `rgb(${l.color[0]}, ${l.color[1]}, ${l.color[2]})`;
  }

  protected down(e: PointerEvent): void {
    (e.target as Element).setPointerCapture(e.pointerId);
    this.drag = { id: e.pointerId, x: e.clientX, y: e.clientY };
  }

  protected move(e: PointerEvent): void {
    if (!this.drag || this.drag.id !== e.pointerId) return;
    const dx = e.clientX - this.drag.x;
    const dy = e.clientY - this.drag.y;
    this.drag = { id: e.pointerId, x: e.clientX, y: e.clientY };
    if (e.buttons & 2 || e.shiftKey) this.store.pan(dx, dy);
    else this.store.orbit(dx, dy);
  }

  protected up(e: PointerEvent): void {
    if (this.drag?.id === e.pointerId) this.drag = undefined;
  }

  protected wheel(e: WheelEvent): void {
    e.preventDefault();
    const px = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
    this.store.zoom(-px);
  }
}

/** Range image of a scan with the current profile marked; click or drag to pick a profile. */
@Component({
  selector: 'll-range-image',
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <canvas #canvas (pointerdown)="pick($event, true)" (pointermove)="pick($event, false)"></canvas>
    <div class="marker" [style.left.%]="marker()"></div>
  `,
  styles: [
    THEME,
    `
      :host { display: block; position: relative; height: 130px; background: #000; cursor: crosshair; touch-action: none; }
      canvas { display: block; width: 100%; height: 100%; image-rendering: auto; }
      .marker { position: absolute; top: 0; bottom: 0; width: 1.5px; background: var(--_visible); pointer-events: none; }
    `,
  ],
})
export class LlRangeImage {
  readonly scan = input.required<Scan>();
  readonly row = input(0);
  readonly rowChange = output<number>();
  private readonly canvas = viewChild.required<ElementRef<HTMLCanvasElement>>('canvas');
  protected readonly marker = computed(() => ((this.row() + 0.5) / this.scan().rows) * 100);

  constructor() {
    // Redraws whenever the scan changes.
    afterRenderEffect({
      write: () => {
        const img = this.scan().rangeImage();
        const canvas = this.canvas().nativeElement;
        canvas.width = img.width;
        canvas.height = img.height;
        canvas.getContext('2d')?.putImageData(img, 0, 0);
      },
    });
  }

  protected pick(e: PointerEvent, start: boolean): void {
    if (!start && !(e.buttons & 1)) return;
    if (start) (e.target as Element).setPointerCapture(e.pointerId);
    const r = (e.target as Element).getBoundingClientRect();
    const rows = this.scan().rows;
    this.rowChange.emit(Math.min(Math.max(Math.floor(((e.clientX - r.left) / r.width) * rows), 0), rows - 1));
  }
}

/** Height along the line for one profile: seen parts in one colour, occluded in another. */
@Component({
  selector: 'll-profile-plot',
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <svg [attr.viewBox]="'0 0 ' + width + ' 100'" preserveAspectRatio="none" role="img" aria-label="Profile">
      @for (r of runs(); track $index) {
        <polyline [attr.points]="r.points" [class.occluded]="!r.visible" />
      }
    </svg>
    <span class="caption">{{ caption() }}</span>
  `,
  styles: [
    THEME,
    `
      :host { display: block; position: relative; height: 100px; background: var(--_inset); border-radius: var(--_radius); }
      svg { width: 100%; height: 100%; display: block; }
      polyline { fill: none; stroke: var(--_visible); stroke-width: 1.5; vector-effect: non-scaling-stroke; }
      polyline.occluded { stroke: var(--_occluded); }
      .caption { position: absolute; left: 4px; top: 2px; font-size: 10px; color: var(--_weak); }
    `,
  ],
})
export class LlProfilePlot {
  readonly scan = input.required<Scan>();
  readonly row = input(0);
  protected readonly width = 1000;

  protected readonly runs = computed(() => {
    const scan = this.scan();
    const cols = Math.max(scan.cols, 2) - 1;
    const zTop = Math.max(scan.zMaxMm * 1.1, 1e-3);
    // Leave room for the caption at the top, like the native plot.
    const y = (z: number) => 96 - (z / zTop) * 82;
    return scan.profileRuns(this.row()).map((r) => {
      let points = '';
      for (let i = 0; i < r.points.length; i += 2) {
        points += `${((r.points[i] / cols) * this.width).toFixed(1)},${y(r.points[i + 1]).toFixed(2)} `;
      }
      return { visible: r.visible, points };
    });
  });

  protected readonly caption = computed(() => {
    const s = this.scan();
    return `profile ${this.row()} · width ${s.widthMm.toFixed(1)} mm · max ${s.zMaxMm.toFixed(2)} mm`;
  });
}
