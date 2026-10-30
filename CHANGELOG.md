# Changelog

## Unreleased

### Added
- CPU renderer for the 2D laser line in `laserline_core::cpu` (`render_gray`, `render_gray8`,
  `render_gray_region`, `render_gray8_region`, `eval_pixel_gray`, `eval_pixel_rgb`), matching
  `laserline-render`'s `laser.wgsl` shader (intensity, width, Gaussian/top-hat/hard profile,
  deformation, speckle, ambient light and sensor noise, with colour/wavelength reduced to
  intensity) for headless consumers.
- The slicer imports 3MF, OBJ, PLY (ASCII and binary), glTF/GLB and STEP (`.step`/`.stp`)
  besides STL, natively and in the browser (button and drop). 3MF reads components and
  Bambu Studio / PrusaSlicer packages (`p:path`); glTF needs embedded buffers. STEP is
  tessellated with truck behind the `step` feature of `laserline-core` (on in the desktop
  app and the wasm build). Files that declare a length unit (3MF, glTF, STEP) set the slice
  settings' unit; a later model without one gets the previous unit back.
  Core: `laserline_core::formats` (`parse_model`, `ModelFormat`, `EXTENSIONS`),
  `Mesh::unit`. JS: `Mesh.parse` / `fromArrays` / `unit`, `constants().modelExtensions`,
  `MODEL_EXTENSIONS`, `MODEL_ACCEPT`, `isModelFile`; the slicing worker parses the file, so
  `SliceRunner.loadModel` is async and returns `{ triangles, unit }`, and
  `SliceWorkerClient` gains `parse` while `setModel` takes the parsed arrays.
  Renamed: `LaserlineStore.importStl` → `importModel`, `FileKind::Stl` → `FileKind::Model`,
  `LaserUi::import_stl` → `import_model`.
- `models/l-block.stl`: a stepped, asymmetric L with known dimensions (`models/README.md`)
  for checking a metric scan against its model, also as `.obj`, `.ply`, `.glb`, `.3mf`
  (exported by Bambu Studio) and `.step`.
- Split into libraries and apps: `laserline-core`, `laserline-render`, `laserline-egui`
  (crates.io), `@javerik/laserline` and `@javerik/laserline-angular` (npm), the native
  `laserline-desktop` app and the `laserline-web` Angular app.
- Browser support: wasm32 build of the core, WebGPU and WebGL2 rendering, slicing in a
  Web Worker.
- The live laser view can be detached into a window of its own and docked back (V, or
  the Detach / Dock buttons; closing its window docks it), natively and in the browser:
  `laserline_egui::show_window`, `DetachedLaserView` and `LaserView.createOffscreen`
  in `@javerik/laserline`, `LaserlineStore.detachView()` / `dockView()` and a detach
  button on `<ll-laser-view>`. Fullscreen applies to the window showing the view.
- The web slicer can open in a window of its own as well (as the native one does):
  `LaserlineStore.detachSlicer()` / `dockSlicer()`, `<ll-window-portal>` to render
  templates into another window, and `openPopup`, `CanvasBridge` and
  `MeshView.createOffscreen` in `@javerik/laserline`.
- Everything the apps can do is library API, in Rust and in JavaScript:
  - Window layout and shortcuts: `laserline_egui::{Layout, Action}` with
    `LaserUi::apply`; `actionForKey` / `SHORTCUTS` in `@javerik/laserline`;
    `LaserlineStore.dispatch()`, `controls()` and `toggleControlsDetached()` /
    `toggleControlsHidden()`. The web app gains D (detach controls), the desktop app S
    (slicer).
  - New JS bindings: `constants()`, `gridSize`, `toScanFrame`, `sampleDeformation`,
    `motionOffset`, `lineAngle`, `extentAlong`, `niceStep`, `Mesh.vertices()` /
    `triangles()`, `Scan.heights()` / `visibility()` / `rowVisible()` / `seen()` /
    `uCenterMm`, `Slicer.clearModel()`, and an `isCancelled` callback for `slice`.
- Camera calibration support: another page of the same origin can drive the line over a
  `BroadcastChannel` (`RemoteHost` in `@javerik/laserline`, `provideLaserline({ remote: true })`,
  on in `laserline-web`). It reads the view size and the line's envelope, shows
  calibration patterns (a blank view, lines across it) while the user's parameters stay
  aside, and reports the part of the view its camera sees. That field is outlined over the
  view, and "Fit line to camera" (`LaserlineStore.fitToCameraField()`) moves and shrinks
  the line into it. Core: `laserline_core::calibration` (`line_envelope`,
  `calibration_params`, `pattern_params`, `fit_into`); JS: `Emulator.envelope` /
  `fitInto`, `calibrationParams`, `patternParams`, `DetachedLaserView.setOverlay`.
  Patterns include bands (`width_pct`), a white view and Gray-code stripes drawn over the view
  (`OverlayPattern`, `drawOverlayPattern`, `stripeLit`, `DetachedLaserView.setPattern`,
  `LaserlineStore.overlayPattern`), so a camera seeing only part of the display can be mapped.
- A controller can limit where the line goes: `constrain { polygon, label }` (answered with
  `constrained { clamped }`, released with `null`) keeps the line, its motion, deformation and
  scan heights inside a convex area. Every parameter change is adjusted as little as needed
  (moved towards the area's middle, then length and motion shrunk, last the heights), so
  controls and presets stop at the limit. The limit is kept across reloads, outlined solid over
  the view and reported in `status.constraint`; *Release limit* lets the line go. Core:
  `calibration::constrain_into`; JS: `Emulator.constrainInto`, `RemoteTarget.constrain`,
  `DetachedLaserView.setLimit`, `LaserlineStore.constraint` / `setConstraint()` / `clamped`.
- A controller can tilt the line: `line` patterns take a `tilt_deg` (`Pattern::Line`), so a
  camera application can find the tilt at which it sees the line level, and
  `tilt { tilt_deg }` (answered with `tilted { tilt_deg }`, clamped to ±45°) sets the user's
  line tilt, applied when a running session ends. `status.tilt_deg` reports it. JS:
  `RemoteTarget.setTilt`, `LaserlineStore.setTilt()`.
- Laserline publishes its metric mapping: `status.metric` (`RemoteMetric`) holds the base line, view px
  per mm along the line and in height (signed), the u at its centre, the travel per row and its speed
  (`per_second` exact, `per_frame` from the measured frame rate), the object's extent, whether the scan
  mirrors the model, and what disturbs it (`motion`, `bounce`, `once`, `paused`, `noise`, `flat`), with a
  `rev` that changes with it but not with frame-rate jitter. It always describes the user's line, also
  during a calibration session. Core: `laserline_core::metric` (`metric_mapping`), `Scan::object_bounds`,
  `mesh::scan_frame_mirrored`, `model::line_length_px`; JS: `Emulator.metric`.
- The default laser colour is white (`ColorParams` defaults to `Custom` 1,1,1); the coloured
  presets set their wavelength. Calibration lines are white.
- Every public Rust item and every TypeScript export is documented; `missing_docs` and
  `scripts/check-dts-docs.mjs` (part of `npm test`) enforce it, and the crate READMEs
  are compiled as doctests.

### Changed
- `LaserUi::begin_frame` clamps `dt` to `MAX_FRAME_DT` (0.25 s). The desktop app
  stores its layout under `layout` (the old `dock` / `view_placement` keys are read once).
- `profileCurve`'s parameter is `widthPx`; the wasm `start` hook is no longer typed.
- The line's deformation samples are read from a texture instead of a storage buffer
  (WebGL2 has none).
- Loading a scan file abandons a slice still running for the replaced model.

### Fixed
- `Scan.rowHeights` / `profileRuns` no longer panic on a scan without rows.
