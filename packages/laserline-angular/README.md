# @javerik/laserline-angular

Angular 21 components for the laserline emulator: the live laser view, the full control
panel and the slicer of 3D models (STL, 3MF, OBJ, PLY, glTF, STEP) with 3D preview, built on
[`@javerik/laserline`](https://www.npmjs.com/package/@javerik/laserline).

## Setup

1. Copy the WebAssembly module into your assets (`angular.json`, build options):

   ```json
   { "glob": "laserline_bg.wasm", "input": "node_modules/@javerik/laserline/wasm", "output": "laserline" }
   ```

2. Add a slicing worker (`src/app/slice.worker.ts`, and `"webWorkerTsConfig"` in
   `angular.json`):

   ```ts
   import { serveSlicing } from '@javerik/laserline/worker';
   serveSlicing();
   ```

3. Provide the store:

   ```ts
   provideLaserline({
     wasmUrl: 'laserline/laserline_bg.wasm',
     createWorker: () => new Worker(new URL('./slice.worker', import.meta.url), { type: 'module' }),
   });
   ```

4. Use the components:

   ```html
   <ll-laser-view />
   <ll-controls />
   @if (store.slicerOpen()) { <ll-slicer /> }
   ```

`<ll-laser-view>` can be detached into a window of its own (its hover button, or
`store.detachView()` / `store.toggleViewDetached()` from a click or key handler) and
docked back (`store.dockView()`, the window's Dock button, or closing the window);
it shows a placeholder meanwhile. Key presses in that window reach the page's
`document` keydown handlers, and `store.toggleFullscreen()` / `store.fullscreen()`
follow whichever window shows the view.

The slicer can open in a window of its own too: `store.detachSlicer()` (from a user
action) opens the window, and `<ll-window-portal>` renders any template into it with
the page's styles. Closing that window closes the slicer, and `store.dockSlicer()` puts
the slicer back into the page:

```html
@if (store.slicerWindow(); as win) {
  <ll-window-portal [window]="win"><ng-template><ll-slicer /></ng-template></ll-window-portal>
} @else if (store.slicerOpen()) {
  <ll-slicer />
}
```

Laserline views inside such windows render through an `OffscreenCanvas` automatically.

The controls follow the same pattern: `store.controls()` is `'docked'`, `'detached'`
(render them into `store.controlsWindow()` with `<ll-window-portal>`) or `'hidden'`;
`store.toggleControlsDetached()` and `store.toggleControlsHidden()` switch it.

Keyboard shortcuts, shared with the desktop app, take one line:

```ts
import { actionForKey } from '@javerik/laserline';

@HostListener('document:keydown', ['$event'])
key(e: KeyboardEvent) {
  const action = actionForKey(e);
  if (action && this.store.dispatch(action)) e.preventDefault();
}
```

The layout is not persisted: windows can only be opened from a user action, so a page
always starts docked.

With `provideLaserline({ …, remote: true })` other pages of the origin can calibrate a
camera against the line (`RemoteHost` in `@javerik/laserline`). While one does,
`store.calibrating()` is true and parameter edits wait for it to end; afterwards
`store.cameraField()` holds the part of the view the camera sees, outlined over
`<ll-laser-canvas>` and the detached view, and `<ll-camera-field>` (inside `<ll-controls>`)
offers `store.fitToCameraField()`. A page can also limit the line to an area
(`store.constraint()`, set remotely or with `store.setConstraint()`): every parameter change
is kept inside it (`store.clamped()` says what gave way) until *Release limit*.

Smaller pieces (`ll-laser-canvas`, `ll-line-controls`, `ll-mesh-view`, `ll-range-image`,
`ll-slider`, …) and the `LaserlineStore` are exported for custom layouts.

## Theming

Components take colours, fonts and radius from CSS custom properties, with dark defaults:
`--ll-bg`, `--ll-surface`, `--ll-inset`, `--ll-border`, `--ll-text`, `--ll-text-weak`,
`--ll-accent`, `--ll-on-accent`, `--ll-error`, `--ll-warn`, `--ll-profile-visible`,
`--ll-profile-occluded`, `--ll-radius`, `--ll-font`, `--ll-font-mono`, `--ll-font-size`,
`--ll-view-bg`.

Licensed under either of Apache-2.0 or MIT, at your option.
