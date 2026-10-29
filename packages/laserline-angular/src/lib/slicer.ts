import { ChangeDetectionStrategy, Component, computed, inject, input, model } from '@angular/core';
import { type FullSliceSettings, MODEL_ACCEPT } from '@javerik/laserline';
import { LlProfileStepper, LlScanSummary } from './panels';
import { LaserlineStore } from './store';
import { LlMeshView, LlProfilePlot, LlRangeImage } from './views';
import { type Choice, LlChoice, LlFileButton, LlSection, LlSlider, THEME } from './widgets';

type S = FullSliceSettings;
const AXES = ['X', 'Y', 'Z'] as const;
const UP: { value: S['up_axis']; label: string; index: number }[] = [
  { value: 'PosX', label: '+X', index: 0 },
  { value: 'NegX', label: '−X', index: 0 },
  { value: 'PosY', label: '+Y', index: 1 },
  { value: 'NegY', label: '−Y', index: 1 },
  { value: 'PosZ', label: '+Z', index: 2 },
  { value: 'NegZ', label: '−Z', index: 2 },
];

/** Orientation, scale, resolution and sensor settings of the slicer. */
@Component({
  selector: 'll-slice-settings',
  imports: [LlSection, LlSlider, LlChoice],
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    @let s = value();
    <ll-section heading="Orientation" [resettable]="false">
      <div class="grid">
        <span>Up axis</span>
        <select aria-label="Up axis" title="Model axis pointing towards the sensor" [value]="s.up_axis" [disabled]="disabled()" (change)="setUp($any($event.target).value)">
          @for (u of up; track u.value) {
            <option [value]="u.value">{{ u.label }}</option>
          }
        </select>
        <span>Travel axis</span>
        <ll-choice [value]="s.travel_axis" (valueChange)="set('travel_axis', $event)" [options]="travelAxes()" [disabled]="disabled()" />
        <span>Along line</span>
        <label title="The remaining axis runs along the laser line; tick to mirror it">
          <input type="checkbox" [checked]="s.flip_along" [disabled]="disabled()" (change)="set('flip_along', $any($event.target).checked)" />
          {{ alongAxis() }} (mirrored)
        </label>
        <span>Rotation</span>
        <div class="row nowrap">
          <ll-slider label="Rotation" [value]="s.yaw_deg" (valueChange)="set('yaw_deg', $event)" [min]="-180" [max]="180" suffix="°" [decimals]="1" [disabled]="disabled()" />
          <button type="button" [disabled]="disabled()" (click)="set('yaw_deg', 0)">0</button>
        </div>
      </div>
    </ll-section>
    <ll-section heading="Scale" [resettable]="false">
      <div class="grid">
        <span>Model unit</span>
        <ll-choice [value]="s.unit" (valueChange)="set('unit', $event)" [options]="units" [disabled]="disabled()" />
        <span>Scale</span>
        <div class="row nowrap">
          <ll-slider label="Scale" [value]="s.scale" (valueChange)="set('scale', $event)" [min]="0.001" [max]="1000" [log]="true" suffix="×" [decimals]="3" [clamp]="true" [disabled]="disabled()" />
          <button type="button" [disabled]="disabled()" (click)="set('scale', 1)">1×</button>
        </div>
        @if (sizeMm(); as d) {
          <span>Size</span>
          <span title="Along line × travel × height">{{ d[0].toFixed(1) }} × {{ d[1].toFixed(1) }} × {{ d[2].toFixed(1) }} mm</span>
        }
      </div>
    </ll-section>
    <ll-section heading="Resolution" [resettable]="false">
      <div class="grid">
        <span>Along line</span>
        <ll-slider label="Lateral resolution" title="Sample spacing along the laser line" [value]="s.lateral_res_mm" (valueChange)="set('lateral_res_mm', $event)" [min]="0.005" [max]="10" [log]="true" suffix="mm" [decimals]="3" [disabled]="disabled()" />
        <span>Travel step</span>
        <ll-slider label="Travel step" title="Object movement between two profiles" [value]="s.travel_step_mm" (valueChange)="set('travel_step_mm', $event)" [min]="0.005" [max]="50" [log]="true" suffix="mm" [decimals]="3" [disabled]="disabled()" />
        <span>Side margin</span>
        <ll-slider label="Side margin" [value]="s.margin_lateral_mm" (valueChange)="set('margin_lateral_mm', $event)" [min]="0" [max]="500" suffix="mm" [decimals]="1" [disabled]="disabled()" />
        <span>Lead in/out</span>
        <ll-slider label="Lead in/out" title="Empty travel before and after the object" [value]="s.margin_travel_mm" (valueChange)="set('margin_travel_mm', $event)" [min]="0" [max]="500" suffix="mm" [decimals]="1" [disabled]="disabled()" />
      </div>
      @if (grid(); as g) {
        @if ('error' in g) {
          <span class="error">{{ g.error }}</span>
        } @else {
          <span class="weak">{{ g.cols }} samples × {{ g.rows }} profiles · {{ (g.bytes / 1e6).toFixed(1) }} MB</span>
        }
      }
    </ll-section>
    <ll-section heading="Sensor" [resettable]="false">
      <div class="grid">
        <span>Conveyor</span>
        <label title="Keep the line visible next to the object, as on a belt">
          <input type="checkbox" [checked]="s.base_plane" [disabled]="disabled()" (change)="set('base_plane', $any($event.target).checked)" /> Plane at z = 0
        </label>
        <span>Triangulation</span>
        <ll-slider label="Triangulation angle" title="Angle between camera axis and laser plane; also sets the default height gain" [value]="s.triangulation_deg" (valueChange)="set('triangulation_deg', $event)" [min]="0" [max]="80" suffix="°" [decimals]="1" [disabled]="disabled()" />
        <span>Occlusion</span>
        <label><input type="checkbox" [checked]="s.occlusion" [disabled]="disabled()" (change)="set('occlusion', $any($event.target).checked)" /> Hide what the camera can't see</label>
        <span>Camera side</span>
        <ll-choice [value]="s.camera_side" (valueChange)="set('camera_side', $event)" [options]="sides" [disabled]="disabled()" />
      </div>
    </ll-section>
  `,
  styles: [THEME, `:host { display: flex; flex-direction: column; } select { justify-self: start; } .nowrap { flex-wrap: nowrap; } .nowrap ll-slider { flex: 1; }`],
})
export class LlSliceSettings {
  readonly value = model.required<S>();
  readonly disabled = input(false);
  readonly sizeMm = input<[number, number, number] | undefined>();
  readonly grid = input<{ cols: number; rows: number; bytes: number } | { error: string } | undefined>();

  protected readonly up = UP;
  protected readonly units: Choice<S['unit']>[] = [
    { value: 'Mm', label: 'mm' },
    { value: 'Cm', label: 'cm' },
    { value: 'M', label: 'm' },
    { value: 'Inch', label: 'inch' },
  ];
  protected readonly sides: Choice<S['camera_side']>[] = [
    { value: 'Ahead', label: 'Ahead (+travel)' },
    { value: 'Behind', label: 'Behind' },
  ];
  private readonly upIndex = computed(() => UP.find((u) => u.value === this.value().up_axis)!.index);
  /** Travel axis index, never the up axis (falls back to the next axis, as the slicer does). */
  private readonly travelIndex = computed(() => {
    const t = AXES.indexOf(this.value().travel_axis);
    return t !== this.upIndex() ? t : (this.upIndex() + 1) % 3;
  });
  protected readonly travelAxes = computed<Choice<S['travel_axis']>[]>(() =>
    AXES.map((a, i) => ({ value: a, label: a, disabled: i === this.upIndex() })),
  );
  protected readonly alongAxis = computed(() => AXES[3 - this.upIndex() - this.travelIndex()]);

  protected set<K extends keyof S>(key: K, v: S[K]): void {
    this.value.update((s) => ({ ...s, [key]: v }));
  }

  protected setUp(upAxis: S['up_axis']): void {
    const up = UP.find((u) => u.value === upAxis)!.index;
    this.value.update((s) => {
      // Keep the travel axis valid after the up axis changed.
      const t = AXES.indexOf(s.travel_axis);
      return { ...s, up_axis: upAxis, travel_axis: t !== up ? s.travel_axis : AXES[(up + 1) % 3] };
    });
  }
}

/** The object slicer: model import, settings and progress on the left, previews on the right. */
@Component({
  selector: 'll-slicer',
  imports: [LlSliceSettings, LlFileButton, LlMeshView, LlRangeImage, LlProfilePlot, LlProfileStepper, LlScanSummary],
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    @if (store.ready()) {
      @let sl = store.slicer();
      <aside>
        <div class="head">
          <h2>Object</h2>
          <ll-file-button [accept]="modelAccept" [disabled]="!store.canSlice" (file)="store.importModel($event)">📂 Import model…</ll-file-button>
        </div>
        @if (sl.model; as m) {
          <strong [title]="m.location">{{ m.name }}</strong>
          <span class="weak">
            {{ m.triangles }} triangles · {{ (m.bounds.hi[0] - m.bounds.lo[0]).toPrecision(5) }} × {{ (m.bounds.hi[1] - m.bounds.lo[1]).toPrecision(5) }} ×
            {{ (m.bounds.hi[2] - m.bounds.lo[2]).toPrecision(5) }} model units
          </span>
        } @else if (store.scan()) {
          <span class="weak">Playing a loaded scan; its settings are shown below. Import the model to re-slice.</span>
        } @else {
          <span class="weak">No model loaded. Import a 3D model (STL, 3MF, OBJ, PLY, glTF, STEP) to slice it.</span>
        }
        <ll-slice-settings
          [value]="store.sliceSettings()"
          (valueChange)="store.setSliceSettings($event)"
          [disabled]="!sl.model && !!store.scan()"
          [sizeMm]="sl.sizeMm"
          [grid]="sl.grid"
        />
        <div class="row">
          <label><input type="checkbox" [checked]="store.sliceAuto()" (change)="store.setSliceAuto($any($event.target).checked)" /> Auto re-slice</label>
          <button type="button" [disabled]="!sl.model" (click)="store.sliceNow()">⟳ Slice now</button>
          <button type="button" title="Reset slice settings to defaults" (click)="store.resetSliceSettings()">↺ Reset</button>
        </div>
        @if (sl.busy) {
          <progress [value]="sl.progress ?? 0" max="1">Slicing…</progress>
        } @else if (sl.stale) {
          <span class="weak">Settings changed since the last slice</span>
        } @else if (sl.lastDurationS !== undefined && sl.model) {
          <span class="weak">Sliced in {{ sl.lastDurationS.toFixed(2) }} s</span>
        }
        @if (sl.error; as e) {
          <span class="error">{{ e }}</span>
        }
        @if (store.scan(); as scan) {
          <hr />
          <ll-scan-summary [scan]="scan" />
          <div class="row">
            <button type="button" (click)="store.saveScan()">💾 Save scan…</button>
            <ll-file-button accept=".lscan" (file)="store.loadScan($event)">📂 Load scan…</ll-file-button>
          </div>
        }
      </aside>
      <section class="preview">
        <ll-mesh-view />
        @if (store.scan(); as scan) {
          <div class="row caption">
            <strong>Range image</strong>
            <small class="weak">x: travel · y: along line · magenta: occluded · click to pick a profile</small>
          </div>
          <ll-range-image [scan]="scan" [row]="store.row()" (rowChange)="store.seekRow($event)" />
          <ll-profile-plot [scan]="scan" [row]="store.row()" />
          <ll-profile-stepper [rows]="scan.rows">
            <span class="mono">v = {{ scan.rowV(store.row()).toFixed(2) }} mm</span>
          </ll-profile-stepper>
        }
      </section>
    }
  `,
  styles: [
    THEME,
    `
      :host { display: grid; grid-template-columns: minmax(280px, 340px) minmax(0, 1fr); height: 100%; min-height: 0; background: var(--_bg); }
      aside { display: flex; flex-direction: column; gap: 6px; padding: 8px 10px; overflow-y: auto; border-right: 1px solid var(--_border); }
      .head { display: flex; justify-content: space-between; align-items: center; }
      h2 { margin: 0; font-size: 1.25em; }
      hr { width: 100%; border: 0; border-top: 1px solid var(--_border); }
      progress { width: 100%; accent-color: var(--_accent); }
      .preview { display: flex; flex-direction: column; gap: 6px; padding: 8px; min-width: 0; min-height: 0; }
      ll-mesh-view { flex: 1; min-height: 160px; border-radius: var(--_radius); }
      .mono { font-family: var(--_mono); white-space: nowrap; }
      @media (max-width: 760px) {
        :host { grid-template-columns: 1fr; height: auto; }
        aside { border-right: 0; border-bottom: 1px solid var(--_border); }
        ll-mesh-view { flex: none; height: 320px; }
      }
    `,
  ],
})
export class LlSlicer {
  protected readonly store = inject(LaserlineStore);
  protected readonly modelAccept = MODEL_ACCEPT;
}
