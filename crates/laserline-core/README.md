# laserline-core

Platform-independent core of [laserline](https://git.javerik.dev/opensource/laserthing):
emulating the laser line of a laser-triangulation sensor and slicing 3D models into the
profiles such a sensor measures. No GUI, GPU or file-system dependencies; builds for
native targets and `wasm32-unknown-unknown`.

```rust,no_run
use laserline_core::{formats, scan};
use std::sync::atomic::{AtomicBool, AtomicU32};

// Any of formats::EXTENSIONS; 3MF, glTF and STEP declare their unit.
let model = formats::parse_model(&std::fs::read("part.stl")?, "part.stl")?;
let unit = model.unit.unwrap_or(scan::ModelUnit::Mm);
let settings = scan::SliceSettings { triangulation_deg: 35.0, unit, ..Default::default() };
let scan = scan::slice(&model, &settings, "part.stl", &AtomicBool::new(false), &AtomicU32::new(0))?;
println!("{} profiles of {} samples", scan.meta.rows, scan.meta.cols);
let heights_mm = scan.row_height(0); // NaN where the laser hits nothing
# Ok::<(), Box<dyn std::error::Error>>(())
```

- `params`, `model`, `presets`: line parameters (serde) and the laser model.
- `emulator::Emulator`: simulation clock, scan playback and per-frame shader uniforms.
- `cpu`: CPU renderer (`render_gray`, `render_gray8`) matching the wgpu line shader, for headless cameras.
- `formats`: model files (STL, 3MF, OBJ, PLY, glTF/GLB, STEP) into a `mesh::Mesh`.
- `mesh`, `scan`, `scan_file`: meshes, slicing (threaded natively), `.lscan` files.
- `slicer::Slicer`: interactive slicer state (debounced re-slicing, previews).
- `camera`, `viz`, `gpu_types`: 3D preview geometry for renderers such as `laserline-render`.

Time is always passed in by the host; nothing here reads a clock.

Features: `step` adds STEP import (tessellated with truck; about 1.2 MB of wasm); `wasm`
adds TypeScript definitions for the serde types (tsify); `test-utils` exposes test helpers.

Licensed under either of Apache-2.0 or MIT, at your option.
