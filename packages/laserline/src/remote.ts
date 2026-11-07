import type { Envelope, LengthUnit, MetricMapping } from '../wasm/laserline.js';
import type { RemotePattern } from './pattern.js';

/** Default `BroadcastChannel` name of the remote protocol. */
export const REMOTE_CHANNEL = 'laserline-remote';
/** Protocol version; messages of another version are ignored. */
export const REMOTE_VERSION = 1;
/** A calibration session without a message for this long ends by itself (ms). */
export const REMOTE_IDLE_MS = 15_000;

/** A point in view pixels (the live view's drawing buffer, y down). */
export type ViewPoint = [number, number];

/**
 * How the user's line maps the loaded scan into view px and how fast it travels
 * (`Emulator.metric`), with a revision: `rev` changes whenever any other field does, but not
 * with frame-rate jitter (in `per_frame` mode the rate is held until it moves by more than 1 %).
 * During a calibration session it stays the user's line, not the pattern shown.
 */
export type RemoteMetric = MetricMapping & {
  /** Changes with every change of the mapping; starts at 1. */
  rev: number;
};

/** Where the controls of a laserline window are: `none` in the controls-free view (`?view=detached`). */
export type RemoteControls = 'docked' | 'detached' | 'hidden' | 'none';

/**
 * The screen the rendering window is on (CSS px of the virtual desktop). `label` and `index`
 * (into `getScreenDetails().screens`) come from the Window Management API and are only
 * present once this origin was granted `window-management`.
 */
export interface RemoteScreen {
  /** Left edge on the virtual desktop. */
  left: number;
  /** Top edge on the virtual desktop. */
  top: number;
  /** Width of the screen. */
  width: number;
  /** Height of the screen. */
  height: number;
  /** The desktop spans more than one screen (`screen.isExtended`), if the browser says. */
  isExtended?: boolean;
  /** The screen's name, e.g. "DELL U2720Q" (Window Management API). */
  label?: string;
  /** Index into `getScreenDetails().screens` (Window Management API). */
  index?: number;
}

/** What a laserline window reports about itself, see {@link RemoteHost}. */
export interface RemoteStatus {
  /** Size of the live view in px; `[0, 0]` before its first frame. */
  viewPx: ViewPoint;
  /** Area the line covers over its whole motion, with the user's parameters; `null` before the first frame. */
  envelope: Envelope | null;
  /** Session of the calibration running now, if any. */
  session: string | null;
  /** The part of the view a camera sees, as last set by a controller. */
  field: ViewPoint[] | null;
  /** The area the line is kept in, as last set by a controller (`constrain`). */
  constraint: ViewPoint[] | null;
  /** The line's tilt in degrees on top of its orientation (the user's parameters). */
  tilt_deg: number;
  /**
   * The metric mapping of the user's line; `null` without a scan, with the scan disabled or
   * before the first frame. Missing from a laserline that can't publish it.
   */
  metric?: RemoteMetric | null;
  /**
   * The window rendering the live view (the detached one, or the page) is visible: not
   * minimised, covered by another tab or hidden. Missing from an older laserline; this and the
   * fields below are optional, so a controller written before them ignores them.
   */
  visible?: boolean;
  /** The window rendering the live view is fullscreen. */
  fullscreen?: boolean;
  /** The Object Slicer is open (drawn over the view unless in a window of its own). */
  slicerOpen?: boolean;
  /** Where the controls are. */
  controls?: RemoteControls;
  /** The screen the window rendering the live view is on; `null` when unknown. */
  screen?: RemoteScreen | null;
}

/**
 * Messages to a laserline window. `to` addresses one window (its `host` id from a
 * status); `hello` goes to all of them.
 */
export type ToLaserline =
  | { v: 1; type: 'hello' }
  | { v: 1; type: 'begin'; to: string; session: string }
  | { v: 1; type: 'show'; to: string; session: string; id: number; pattern: RemotePattern }
  | { v: 1; type: 'end'; to: string; session: string }
  | { v: 1; type: 'camera-field'; to: string; polygon: ViewPoint[] | null; label?: string }
  | { v: 1; type: 'fit-to-field'; to: string; id: number; margin: number }
  | { v: 1; type: 'constrain'; to: string; id: number; polygon: ViewPoint[] | null; label?: string }
  | { v: 1; type: 'tilt'; to: string; id: number; tilt_deg: number }
  | { v: 1; type: 'reset-length'; to: string; id: number };

/** Messages from a laserline window; `host` identifies it. */
export type FromLaserline =
  | { v: 1; type: 'status'; host: string; status: RemoteStatus }
  | { v: 1; type: 'shown'; host: string; session: string; id: number; viewPx: ViewPoint }
  | { v: 1; type: 'fitted'; host: string; id: number; fits: boolean }
  | { v: 1; type: 'constrained'; host: string; id: number; clamped: string[] }
  | { v: 1; type: 'tilted'; host: string; id: number; tilt_deg: number }
  | { v: 1; type: 'length-reset'; host: string; id: number; length: number; length_unit: LengthUnit }
  | { v: 1; type: 'error'; host: string; session?: string; id?: number; message: string }
  | { v: 1; type: 'bye'; host: string };

/** What a {@link RemoteHost} drives: the emulator and its view, e.g. `LaserlineStore`. */
export interface RemoteTarget {
  /** The current status. */
  status(): Omit<RemoteStatus, 'session'>;
  /** Keep the user's parameters aside and stop the simulation for patterns. */
  begin(): void;
  /** Draw `pattern`, then call `rendered` once it has been on screen for a frame. */
  show(pattern: RemotePattern, rendered: () => void): void;
  /** Put the user's parameters and playback back. */
  end(): void;
  /** Show (or with `null`, remove) the outline of the camera's field. */
  setField(polygon: ViewPoint[] | null, label: string | null): void;
  /** Fit the line into the field `margin` px inside its edges; whether it fits, `undefined` without a field. */
  fitToField(margin: number): boolean | undefined;
  /**
   * Keep the line inside the convex `polygon` from now on (`null` releases it); returns the
   * names of the parameters changed to fit (empty when none were, or on release).
   */
  constrain(polygon: ViewPoint[] | null, label: string | null): string[];
  /** Set the user's line tilt (degrees, clamped to ±45); returns the tilt kept. */
  setTilt(deg: number): number;
  /** Set the user's line to the view's full length; returns the length kept (a limit may shorten it). */
  resetLength(): { length: number; length_unit: LengthUnit };
}

/** Options of {@link RemoteHost}. */
export interface RemoteHostOptions {
  /** Channel name, {@link REMOTE_CHANNEL} by default. */
  channel?: string;
  /** See {@link REMOTE_IDLE_MS}. */
  idleMs?: number;
  /**
   * Where `pagehide`/`pageshow` are listened for (the page's `window` by default, when there
   * is one): on `pagehide` the host ends a running session and says `bye`; on `pageshow` of a
   * page restored from the back/forward cache it sends its status again.
   */
  page?: Pick<EventTarget, 'addEventListener' | 'removeEventListener'> | null;
}

/**
 * Lets another page of the same origin drive this laserline window over a
 * `BroadcastChannel`, e.g. a camera application calibrating against the emulated line:
 *
 * 1. `hello` → every window answers with its `status` (view size, the line's envelope).
 * 2. `begin` → the window keeps the user's parameters aside and pauses.
 * 3. `show` patterns (a blank view, lines across it) → `shown` once each is on screen.
 * 4. `end` → the user's parameters and playback come back. A session also ends when the
 *    controller goes quiet for {@link REMOTE_IDLE_MS}.
 * 5. `camera-field` → the window outlines the part of its view the camera sees, and
 *    `fit-to-field` fits the line into it.
 * 6. `constrain` → the window keeps the line, its motion, deformation and scan heights inside
 *    an area until released with `null`, answering `constrained` with what it had to change.
 * 7. `tilt` → the window sets the user's line tilt (applied when a running session ends),
 *    answering `tilted` with the tilt kept. A `line` pattern takes its own `tilt_deg`, so a
 *    controller can find the tilt at which the camera sees the line level.
 * 8. `reset-length` → the window sets the user's line to the view's full length (100 %, applied
 *    when a running session ends), answering `length-reset` with the length kept. A controller
 *    sends it before a calibration, since `fit-to-field` and `constrain` only ever shorten it.
 *
 * Every status carries the line's `metric` mapping (view px per mm of the scan, the base
 * line, the travel speed), so a controller can turn what its camera measures into mm, and
 * what the window looks like: `visible`, `fullscreen`, `slicerOpen`, `controls`, `screen`.
 *
 * A window that closes (`pagehide`, or {@link RemoteHost.close}) sends `bye`, so a controller
 * can tell a closed window from a hidden one.
 *
 * Only one session runs at a time; a second `begin` gets an `error`.
 */
export class RemoteHost {
  /** Identifies this window in its messages. */
  readonly id = randomId();
  private readonly channel: BroadcastChannel;
  private readonly idleMs: number;
  private session: string | null = null;
  private lastMessage = 0;
  private idleTimer?: ReturnType<typeof setInterval>;
  private lastStatus = '';
  private closed = false;
  /** When a controller last sent this window something (`hello` included); 0 = never. */
  private lastSeen = 0;
  private readonly page: Pick<EventTarget, 'addEventListener' | 'removeEventListener'> | null;
  private readonly onPageHide = () => this.goodbye();
  private readonly onPageShow = (e: Event) => {
    if ((e as PageTransitionEvent).persisted) this.postStatus(true);
  };

  /** Starts listening. */
  constructor(
    private readonly target: RemoteTarget,
    options: RemoteHostOptions = {},
  ) {
    this.channel = new BroadcastChannel(options.channel ?? REMOTE_CHANNEL);
    this.idleMs = options.idleMs ?? REMOTE_IDLE_MS;
    this.channel.onmessage = (e: MessageEvent) => this.receive(e.data as ToLaserline);
    this.page = options.page !== undefined ? options.page : defaultPage();
    this.page?.addEventListener('pagehide', this.onPageHide);
    this.page?.addEventListener('pageshow', this.onPageShow);
  }

  /** The session running now, if any. */
  get activeSession(): string | null {
    return this.session;
  }

  /**
   * A controller is linked: a session runs, or a controller sent this window something
   * (`hello` included) within the idle time ({@link REMOTE_IDLE_MS}).
   */
  get linked(): boolean {
    return !this.closed && (this.session !== null || (this.lastSeen > 0 && Date.now() - this.lastSeen <= this.idleMs));
  }

  /** Sends the status if it changed since the last one sent, or always with `force`. */
  postStatus(force = false): void {
    if (this.closed) return;
    const status: RemoteStatus = { ...this.target.status(), session: this.session };
    const text = JSON.stringify(status);
    if (!force && text === this.lastStatus) return;
    this.lastStatus = text;
    this.post({ v: REMOTE_VERSION, type: 'status', host: this.id, status });
  }

  /** Stops listening; a running session ends and the window says `bye`. */
  close(): void {
    if (this.closed) return;
    this.goodbye();
    this.closed = true;
    this.page?.removeEventListener('pagehide', this.onPageHide);
    this.page?.removeEventListener('pageshow', this.onPageShow);
    this.channel.close();
  }

  /** Ends a running session and sends `bye` (the window goes away, maybe into the back/forward cache). */
  private goodbye(): void {
    if (this.closed) return;
    if (this.session) this.finish();
    this.lastStatus = '';
    this.post({ v: REMOTE_VERSION, type: 'bye', host: this.id });
  }

  private receive(msg: ToLaserline): void {
    if (this.closed || !msg || msg.v !== REMOTE_VERSION) return;
    if (msg.type === 'hello') {
      this.lastSeen = Date.now();
      this.postStatus(true);
      return;
    }
    if (msg.to !== this.id) return;
    this.lastMessage = Date.now();
    this.lastSeen = this.lastMessage;
    switch (msg.type) {
      case 'begin':
        if (this.session && this.session !== msg.session) {
          this.post({ v: 1, type: 'error', host: this.id, session: msg.session, message: 'another calibration is running' });
          return;
        }
        if (!this.session) {
          this.session = msg.session;
          this.target.begin();
          this.idleTimer = setInterval(() => this.checkIdle(), Math.min(1000, this.idleMs));
        }
        this.postStatus(true);
        break;
      case 'show': {
        if (msg.session !== this.session) {
          this.post({ v: 1, type: 'error', host: this.id, session: msg.session, id: msg.id, message: 'no such session' });
          return;
        }
        const { session, id } = msg;
        this.target.show(msg.pattern, () => {
          if (this.session !== session || this.closed) return;
          this.post({ v: 1, type: 'shown', host: this.id, session, id, viewPx: this.target.status().viewPx });
        });
        break;
      }
      case 'end':
        if (msg.session === this.session) this.finish();
        break;
      case 'camera-field':
        this.target.setField(msg.polygon, msg.label ?? null);
        this.postStatus();
        break;
      case 'fit-to-field': {
        const fits = this.target.fitToField(msg.margin);
        if (fits === undefined) {
          this.post({ v: 1, type: 'error', host: this.id, id: msg.id, message: 'no camera field set' });
        } else {
          this.post({ v: 1, type: 'fitted', host: this.id, id: msg.id, fits });
          this.postStatus();
        }
        break;
      }
      case 'constrain': {
        const clamped = this.target.constrain(msg.polygon?.length ? msg.polygon : null, msg.label ?? null);
        this.post({ v: 1, type: 'constrained', host: this.id, id: msg.id, clamped });
        this.postStatus();
        break;
      }
      case 'tilt': {
        const tilt_deg = this.target.setTilt(msg.tilt_deg);
        this.post({ v: 1, type: 'tilted', host: this.id, id: msg.id, tilt_deg });
        this.postStatus();
        break;
      }
      case 'reset-length': {
        const kept = this.target.resetLength();
        this.post({ v: 1, type: 'length-reset', host: this.id, id: msg.id, ...kept });
        this.postStatus();
        break;
      }
    }
  }

  private checkIdle(): void {
    if (this.session && Date.now() - this.lastMessage > this.idleMs) this.finish();
  }

  private finish(): void {
    clearInterval(this.idleTimer);
    this.idleTimer = undefined;
    this.session = null;
    this.target.end();
    this.postStatus(true);
  }

  private post(msg: FromLaserline): void {
    if (!this.closed) this.channel.postMessage(msg);
  }
}

/** The page's `window` in a browser; nothing in Node or a worker. */
function defaultPage(): Pick<EventTarget, 'addEventListener' | 'removeEventListener'> | null {
  const w = (globalThis as { window?: unknown }).window;
  return typeof w === 'object' && w !== null && typeof (w as EventTarget).addEventListener === 'function'
    ? (w as EventTarget)
    : null;
}

function randomId(): string {
  return globalThis.crypto?.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}
