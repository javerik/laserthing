import { ChangeDetectionStrategy, Component, effect, inject, signal } from '@angular/core';
import { NgTemplateOutlet } from '@angular/common';
import { actionForKey, shortcutHelp } from '@javerik/laserline';
import { LaserlineStore, LlControls, LlLaserView, LlSlicer, LlWindowPortal } from '@javerik/laserline-angular';

/**
 * Web version of the desktop emulator: the live view fills the window (V detaches it
 * into a window of its own and docks it back), the controls are docked on the right
 * (D detaches them, H hides them) and the slicer opens over the view or in a window of
 * its own. Shortcuts and layout come from the library, as in the desktop app.
 */
@Component({
  selector: 'app-root',
  imports: [NgTemplateOutlet, LlLaserView, LlControls, LlSlicer, LlWindowPortal],
  changeDetection: ChangeDetectionStrategy.OnPush,
  host: {
    '(document:keydown)': 'key($event)',
    '(dragover)': '$event.preventDefault()',
    '(drop)': 'drop($event)',
  },
  template: `
    <main [class.hidden]="store.controls() !== 'docked'">
      <ll-laser-view class="live" (activate)="store.toggleFullscreen()" />
      @switch (store.controls()) {
        @case ('hidden') {
          @if (hintVisible()) {
            <div class="hint">Controls hidden — press H to show</div>
          }
        }
        @case ('docked') {
          <aside>
            <ng-container *ngTemplateOutlet="controls" />
          </aside>
        }
        @case ('detached') {
          @if (store.controlsWindow(); as win) {
            <ll-window-portal [window]="win">
              <ng-template>
                <aside class="window"><ng-container *ngTemplateOutlet="controls" /></aside>
              </ng-template>
            </ll-window-portal>
          }
        }
      }
      <ng-template #controls>
        <header>
          <h1>Laser Line</h1>
          <button type="button" title="Hide controls (H)" (click)="store.toggleControlsHidden()">Hide</button>
          <button type="button" title="Toggle live view fullscreen (F11, double-click the view)" (click)="store.toggleFullscreen()">
            {{ store.fullscreen() ? 'Windowed' : 'Fullscreen' }}
          </button>
          <button type="button" [title]="store.viewDetached() ? 'Dock the live view into this page (V)' : 'Show the live view in a window of its own (V)'" (click)="store.toggleViewDetached()">
            {{ store.viewDetached() ? 'Dock view' : '⧉ Detach view' }}
          </button>
          <button type="button" [title]="store.controls() === 'detached' ? 'Dock the controls into the page (D)' : 'Show the controls in a window of their own (D)'" (click)="store.toggleControlsDetached()">
            {{ store.controls() === 'detached' ? 'Dock controls' : '⧉ Detach controls' }}
          </button>
        </header>
        <ll-controls />
        <p class="keys">Keys: {{ keys }} · drop 3D models (STL, 3MF, OBJ, PLY, glTF, STEP) or .lscan files anywhere</p>
      </ng-template>
      @if (store.slicerWindow(); as win) {
        <ll-window-portal [window]="win">
          <ng-template>
            <div class="slicer window">
              <header>
                <h1>Object Slicer</h1>
                <button type="button" title="Show the slicer over the live view in the main window" (click)="store.dockSlicer()">Dock</button>
                <button type="button" aria-label="Close slicer" (click)="store.slicerOpen.set(false)">✕</button>
              </header>
              <ll-slicer />
            </div>
          </ng-template>
        </ll-window-portal>
      } @else if (store.slicerOpen()) {
        <div class="slicer" role="dialog" aria-label="Object slicer">
          <header>
            <h1>Object Slicer</h1>
            <button type="button" title="Open the slicer in a window of its own" (click)="store.detachSlicer()">⧉ Own window</button>
            <button type="button" aria-label="Close slicer" (click)="store.slicerOpen.set(false)">✕</button>
          </header>
          <ll-slicer />
        </div>
      }
    </main>
  `,
  styles: `
    main { position: fixed; inset: 0; display: grid; grid-template-columns: minmax(0, 1fr) 380px; font-family: system-ui, sans-serif; color: #d9dce1; }
    main.hidden { grid-template-columns: 1fr; }
    .live { min-width: 0; min-height: 0; }
    aside { overflow-y: auto; background: #15171a; border-left: 1px solid #31353b; padding: 8px 12px; }
    aside.window { position: fixed; inset: 0; border: 0; font-family: system-ui, sans-serif; color: #d9dce1; }
    header { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
    h1 { font-size: 1.3em; margin: 0 auto 0 0; }
    header button { font: inherit; font-size: 13px; color: inherit; background: #1d2024; border: 1px solid #31353b; border-radius: 3px; padding: 2px 8px; cursor: pointer; }
    .keys { font-size: 11px; color: #8a9099; }
    .hint { position: absolute; left: 0; right: 0; bottom: 24px; text-align: center; color: #a0a0a0; font-size: 16px; pointer-events: none; }
    .slicer { position: absolute; inset: 24px; display: flex; flex-direction: column; background: #15171a; border: 1px solid #31353b; border-radius: 4px; box-shadow: 0 8px 40px #000a; }
    .slicer.window { position: fixed; inset: 0; border: 0; border-radius: 0; box-shadow: none; font-family: system-ui, sans-serif; color: #d9dce1; }
    .slicer header { padding: 6px 10px; border-bottom: 1px solid #31353b; }
    .slicer ll-slicer { flex: 1; min-height: 0; overflow: auto; }
    @media (max-width: 760px) {
      main { grid-template-columns: 1fr; grid-template-rows: 45vh minmax(0, 1fr); }
      aside { border-left: 0; border-top: 1px solid #31353b; }
      .slicer { inset: 0; border-radius: 0; }
    }
  `,
})
export class App {
  protected readonly store = inject(LaserlineStore);
  protected readonly keys = shortcutHelp();
  protected readonly hintVisible = signal(false);
  private hintTimer?: ReturnType<typeof setTimeout>;

  constructor() {
    // Brief hint after hiding the controls, then a clean picture.
    effect(() => {
      const hidden = this.store.controls() === 'hidden';
      clearTimeout(this.hintTimer);
      this.hintVisible.set(hidden);
      if (hidden) this.hintTimer = setTimeout(() => this.hintVisible.set(false), 3000);
    });
  }

  protected key(e: KeyboardEvent): void {
    const action = actionForKey(e);
    if (action && this.store.dispatch(action)) e.preventDefault();
  }

  protected drop(e: DragEvent): void {
    e.preventDefault();
    const file = e.dataTransfer?.files[0];
    if (file && this.store.ready()) void this.store.openFile(file);
  }
}
