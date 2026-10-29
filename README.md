# laserline

Emulates the laser line of a laser-triangulation sensor and slices 3D models into the
profiles such a sensor would measure. Runs natively (egui) and in the browser
(WebAssembly with WebGPU or WebGL2, Angular components).

- **Live line**: intensity, width, Gaussian / top-hat / hard profile, wavelength colour,
  sweep and oscillation, deformation shapes, speckle, ambient light and sensor noise.
- **Object scans**: slice a 3D model (STL, 3MF, OBJ, PLY, glTF/GLB, STEP) as a sensor with
  a given triangulation angle sees it (including camera occlusion), preview it in 3D, and
  play the profiles back through the line. Scans save as `.lscan` files. 3MF, glTF and
  STEP files bring their length unit along; STEP's surfaces are tessellated on import
  (single parts: assembly placements are not applied).
- **Camera calibration** (web): another page of the same origin can drive the line over a
  `BroadcastChannel` to calibrate a camera filming the screen, then outline the camera's
  field and fit the line into it (`RemoteHost`, see `packages/laserline/README.md`). Its
  status says how the line maps a scan's millimetres into view pixels and how fast the scan
  travels, so the camera's measurements can be made metric.

## Layout

| Path | What | Published as |
|---|---|---|
| `crates/laserline-core` | Pure logic: parameters, laser model, model file parsing, slicing, `.lscan`, emulator and slicer state. Native and wasm32. | crates.io `laserline-core` |
| `crates/laserline-render` | wgpu renderers (laser pass, 3D preview); WebGL2-compatible shaders. | crates.io `laserline-render` |
| `crates/laserline-egui` | egui panels and views for native apps. | crates.io `laserline-egui` |
| `crates/laserline-wasm` | wasm-bindgen bindings. | npm `@javerik/laserline` (via `packages/laserline`) |
| `packages/laserline` | TypeScript wrapper: init, render loop, slicing Web Worker, file helpers. | npm `@javerik/laserline` |
| `packages/laserline-angular` | Angular 21 components and store. | npm `@javerik/laserline-angular` |
| `apps/laserline-desktop` | Native app (binary `laserline`). | — |
| `apps/laserline-web` | Angular demo app, the web counterpart of the desktop app. | — |

## Building

Native app:

```sh
cargo run --release -p laserline-desktop -- [model.stl|.3mf|.obj|.ply|.glb|.step | scan.lscan]
```

Web (needs the `wasm32-unknown-unknown` target and the matching `wasm-bindgen-cli`, see
`scripts/build-wasm.sh`; `wasm-opt` is used when installed):

```sh
npm install
npm run build:wasm        # crates/laserline-wasm -> packages/laserline/wasm
npm run build:laserline   # TypeScript wrapper
npm run build:angular     # Angular library
npm start                 # demo app on http://localhost:4200 (?backend=webgl forces WebGL2)
```

## Tests

```sh
cargo test --workspace                      # incl. headless GPU tests on every local backend
npm test -w @javerik/laserline              # the wasm build in Node
npx ng test laserline-angular --no-watch    # Angular library (Vitest)
```

`LASERLINE_GOLDEN=<dir> npm test -w @javerik/laserline` additionally compares wasm slices
byte for byte with `.lscan` files from the native build (`<dir>/cases.json` lists them).

## Publishing

`scripts/publish-dry-run.sh` packages everything without uploading. Crates go out in the
order core → render → egui (`cargo publish --workspace` handles it); the npm packages
after `npm run build:wasm` and the builds above. Crate and npm versions are kept equal.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at
your option.
