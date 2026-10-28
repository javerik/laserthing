# laserline-egui

egui frontend of [laserline](https://git.javerik.dev/opensource/laserthing) for native
applications: the live laser view, the control panel and the slicer of 3D models (STL, 3MF, OBJ, PLY, glTF, STEP) with 3D preview,
range image and profile plot, plus the window layout and keyboard shortcuts of the
desktop app.

- `LaserUi` holds the emulator and the slicer and draws the panels: `live_view`,
  `controls_ui`, `slicer_panels`. The individual widgets (`line_ui`, `profile_ui`, …)
  are public for custom layouts.
- `Layout` records where the controls and the live view are (docked, detached, hidden);
  it is serde, to persist it. `Action` is every shortcut; `Action::from_key` maps keys,
  `LaserUi::apply` runs an action.
- `show_window` shows content in a window of its own (a floating egui window where the
  backend has no multi-viewport support). Keep the live view detachable this way.
- `FileHost` is implemented by the application: file dialogs are its business.

egui must render with wgpu; call `LaserUi::install_gpu` once with the
`egui_wgpu::RenderState` (eframe: `CreationContext::wgpu_render_state`). Slices run on a
background thread, so this crate targets native platforms.

```rust,no_run
use laserline_egui::{Action, FileHost, LaserUi, Layout, ViewPlacement, egui, show_window};

fn frame(ctx: &egui::Context, ui: &mut LaserUi, layout: &mut Layout, files: &mut dyn FileHost, dt: f64) {
    let actions: Vec<Action> = ctx.input(|i| {
        i.events.iter().filter_map(|e| match e {
            egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } => Action::from_key(*key, *modifiers),
            _ => None,
        }).collect()
    });
    for action in actions {
        if let Some(on) = ui.apply(action, layout) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on)); // the live view's window
        }
    }

    ui.begin_frame(ctx, dt);
    egui::SidePanel::right("controls").show(ctx, |p| ui.controls_ui(p, files));
    if layout.view == ViewPlacement::Detached {
        let builder = egui::ViewportBuilder::default().with_title("Live view");
        let open = show_window(ctx, egui::ViewportId::from_hash_of("live"), builder, egui::Frame::NONE, |p| {
            let rect = p.max_rect();
            ui.live_view(p, rect);
        });
        if !open {
            layout.view = ViewPlacement::Docked;
        }
    } else {
        egui::CentralPanel::default().show(ctx, |p| {
            let rect = p.max_rect();
            ui.live_view(p, rect);
        });
    }
}
```

See `apps/laserline-desktop` in the repository for a complete application.

Licensed under either of Apache-2.0 or MIT, at your option.
