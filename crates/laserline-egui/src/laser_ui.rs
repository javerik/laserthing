//! Control panel and live view of the laser line. The same UI is used docked and
//! detached; window management stays with the application.

use crate::files::{FileHost, FileKind, OpenedFile};
use crate::layout::{Action, Layout};
use crate::callbacks::{self, LaserCallback};
use crate::slicer_ui::{profile_plot, scan_summary, unit_label};
use crate::worker::SliceWorker;
use egui::{self, Color32, RichText, Slider, Stroke, Ui};
use laserline_core::emulator::{Emulator, LiveInfo};
use laserline_core::model;
use laserline_core::params::{
    ColorMode, ColorParams, DeformKind, DeformParams, EndMode, HeightGain, LaserParams, LengthUnit, LineParams,
    MotionKind, MotionParams, NoiseParams, Orientation, ProfileKind, ProfileParams, RateMode, ScanParams, ScanScale,
    SweepMode,
};
use laserline_core::scan::{DisplayMapping, Scan, SliceSettings};
use laserline_core::slicer::Slicer;
use laserline_core::formats::{self, ModelFormat};
use laserline_core::{presets, scan_file};
use std::sync::Arc;

const WAVELENGTH_PRESETS: [f32; 5] = [405.0, 450.0, 520.0, 635.0, 660.0];
/// Longest frame step in seconds [`LaserUi::begin_frame`] passes on.
pub const MAX_FRAME_DT: f64 = 0.25;
/// How long a status message stays visible, in seconds.
const STATUS_S: f64 = 5.0;

/// The emulator with its slicer and everything the panels show.
pub struct LaserUi {
    /// The emulator shown in the live view.
    pub emu: Emulator,
    /// The slicer behind the slicer panels.
    pub slicer: Slicer,
    /// The slicer window / panel is shown.
    pub slicer_open: bool,
    /// Measured while drawing the last live view.
    pub live: LiveInfo,
    /// Built-in preset selected in the presets combo box.
    pub preset_index: usize,
    status: Option<(String, f64)>,
    now: f64,
    pub(crate) worker: SliceWorker,
    pub(crate) range_image: Option<(egui::TextureHandle, Arc<Scan>)>,
    srgb_target: bool,
}

impl LaserUi {
    /// Registers the GPU resources of the live and 3D views with egui's wgpu renderer.
    pub fn install_gpu(rs: &egui_wgpu::RenderState) {
        callbacks::install(rs);
    }

    /// `srgb_target`: the render target encodes sRGB itself (`TextureFormat::is_srgb`).
    pub fn new(params: LaserParams, slice: SliceSettings, auto_slice: bool, srgb_target: bool) -> Self {
        Self {
            emu: Emulator::new(params),
            slicer: Slicer::new(slice, auto_slice),
            slicer_open: false,
            live: LiveInfo::default(),
            preset_index: 0,
            status: None,
            now: 0.0,
            worker: SliceWorker::default(),
            range_image: None,
            srgb_target,
        }
    }

    /// The render target encodes sRGB itself.
    pub fn srgb_target(&self) -> bool {
        self.srgb_target
    }

    /// Call once per displayed frame, before drawing: collects finished slices,
    /// starts due re-slices and advances the emulator by `dt` seconds (clamped to
    /// [`MAX_FRAME_DT`], so a stalled window does not jump ahead).
    pub fn begin_frame(&mut self, ctx: &egui::Context, dt: f64) {
        let dt = dt.clamp(0.0, MAX_FRAME_DT);
        self.now = ctx.input(|i| i.time);
        if let Some(scan) = self.worker.poll(&mut self.slicer, self.now) {
            self.emu.set_scan(scan);
        }
        self.emu.tick(dt);
    }

    /// Shows `msg` in the status line for a few seconds.
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), self.now));
    }

    /// The latest status message while it is fresh.
    pub fn status(&self) -> Option<&str> {
        self.status.as_ref().filter(|(_, at)| self.now - at < STATUS_S).map(|(m, _)| m.as_str())
    }

    /// Starts slicing the model with the current settings.
    pub fn slice_now(&mut self) {
        if let Some(req) = self.slicer.start() {
            self.worker.spawn(req);
        }
    }

    /// Loads a 3D model into the slicer and shows it. A unit its file declares becomes
    /// the slice settings' unit.
    pub fn open_model(&mut self, file: OpenedFile) -> Result<(), String> {
        let mesh = formats::parse_model(&file.bytes, &file.name).map_err(|e| format!("{}: {e}", file.location))?;
        let n = mesh.triangles.len();
        let unit = mesh.unit.map_or(String::new(), |u| format!(", unit {} from the file", unit_label(u)));
        if let Some(req) = self.slicer.set_model(mesh, file.name, file.location.clone()) {
            self.worker.spawn(req);
        }
        self.slicer_open = true;
        self.set_status(format!("Imported {} ({n} triangles{unit})", file.location));
        Ok(())
    }

    /// Plays an `.lscan` file.
    pub fn open_scan(&mut self, file: OpenedFile) -> Result<(), String> {
        let scan = scan_file::from_bytes(&file.bytes).map_err(|e| format!("{}: {e}", file.location))?;
        self.adopt_scan(scan);
        self.set_status(format!("Loaded {}", file.location));
        Ok(())
    }

    /// Opens a model or a scan, chosen by the file extension.
    pub fn open_file(&mut self, file: OpenedFile) -> Result<(), String> {
        let ext = file.name.rsplit_once('.').map_or("", |(_, e)| e).to_ascii_lowercase();
        if ext == scan_file::EXTENSION {
            self.open_scan(file)
        } else if ModelFormat::from_name(&file.name).is_some() {
            self.open_model(file)
        } else {
            Err(format!("{}: expected a 3D model (STL, 3MF, OBJ, PLY, glTF, STEP) or an .lscan file", file.location))
        }
    }

    /// Plays a scan loaded from a file. It replaces the model so the previews match what
    /// plays back, and its settings are shown (and reused if the model is imported again).
    pub fn adopt_scan(&mut self, scan: Scan) {
        self.slicer.clear_model();
        self.slicer.settings = scan.meta.settings.clone();
        self.emu.pos = 0.0;
        self.emu.set_scan(scan);
    }

    /// Asks for a 3D model and loads it into the slicer.
    pub fn import_model(&mut self, files: &mut dyn FileHost) {
        let result = files.open(FileKind::Model).and_then(|f| f.map_or(Ok(()), |f| self.open_model(f)));
        if let Err(e) = result {
            self.set_status(format!("Import failed: {e}"));
        }
    }

    /// Asks for an `.lscan` file and plays it.
    pub fn load_scan(&mut self, files: &mut dyn FileHost) {
        let result = files.open(FileKind::Scan).and_then(|f| f.map_or(Ok(()), |f| self.open_scan(f)));
        if let Err(e) = result {
            self.set_status(format!("Load failed: {e}"));
        }
    }

    /// Asks where to save the scan being played.
    pub fn save_scan(&mut self, files: &mut dyn FileHost) {
        let Some(scan) = &self.emu.scan else { return };
        let result = scan_file::to_bytes(scan).and_then(|b| files.save(FileKind::Scan, &scan_file::file_name(scan), &b));
        match result {
            Ok(Some(path)) => self.set_status(format!("Saved {path}")),
            Ok(None) => {}
            Err(e) => self.set_status(format!("Save failed: {e}")),
        }
    }

    /// Runs `action`, changing `layout` for the window actions. Returns `Some(on)` when
    /// the live view's window should enter or leave fullscreen; the host sends that to
    /// the window and mirrors the result in [`Layout::fullscreen`].
    pub fn apply(&mut self, action: Action, layout: &mut Layout) -> Option<bool> {
        match action {
            Action::TogglePlay => self.emu.playing = !self.emu.playing,
            Action::ResetTime => self.emu.reset_time(),
            Action::StepBack => self.emu.step_scan(-1),
            Action::StepForward => self.emu.step_scan(1),
            Action::ToggleFullscreen => return Some(!layout.fullscreen),
            Action::ExitFullscreen => return layout.fullscreen.then_some(false),
            Action::ToggleControlsDetached => layout.toggle_controls_detached(),
            Action::ToggleViewDetached => return layout.toggle_view_detached().then_some(false),
            Action::ToggleControlsHidden => layout.toggle_controls_hidden(),
            Action::ToggleSlicer => self.slicer_open = !self.slicer_open,
        }
        None
    }

    /// Handles the playback keys (Space, R, ←, →); returns whether `key` was one.
    /// Hosts with window actions use [`Action::from_key`] and [`LaserUi::apply`] instead.
    pub fn handle_key(&mut self, key: egui::Key) -> bool {
        match Action::from_key(key, egui::Modifiers::NONE) {
            Some(a @ (Action::TogglePlay | Action::ResetTime | Action::StepBack | Action::StepForward)) => {
                self.apply(a, &mut Layout::default());
                true
            }
            _ => false,
        }
    }

    /// Draws the laser line into `rect` and records the measurements in `self.live`.
    pub fn live_view(&mut self, ui: &mut Ui, rect: egui::Rect) {
        let ppp = ui.ctx().pixels_per_point();
        let origin = [rect.min.x * ppp, rect.min.y * ppp];
        let size = [rect.width() * ppp, rect.height() * ppp];
        let frame = self.emu.frame(origin, size, self.srgb_target);
        self.live = frame.info;
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            LaserCallback { uniforms: frame.uniforms, deform: frame.deform },
        ));
    }

    /// Transport, all parameter sections, object scan, presets and readouts.
    pub fn controls_ui(&mut self, ui: &mut Ui, files: &mut dyn FileHost) {
        ui.spacing_mut().slider_width = 170.0;
        self.transport_ui(ui);
        ui.separator();

        let extent = self.live.extent_along_px;
        let length_px = self.live.length_px;
        let p = &mut self.emu.params;
        section(ui, "Line", true, &mut p.line, |ui, l| line_ui(ui, l, extent));
        let width = p.line.width_px;
        section(ui, "Profile", true, &mut p.profile, |ui, pr| profile_ui(ui, pr, width));
        section(ui, "Color", false, &mut p.color, color_ui);
        let offset = self.live.offset_px;
        section(ui, "Movement", true, &mut p.motion, |ui, m| motion_ui(ui, m, offset));
        let t = self.emu.sim_time;
        let scan_drives = self.emu.scan_drives_line();
        let p = &mut self.emu.params;
        section(ui, "Deformation", false, &mut p.deform, |ui, d| {
            if scan_drives {
                ui.label(RichText::new("The object scan shapes the line; this deformation is inactive.").weak());
            }
            deform_ui(ui, d, length_px, t)
        });
        section(ui, "Background & noise", false, &mut p.noise, noise_ui);

        egui::CollapsingHeader::new(RichText::new("Object scan").strong())
            .default_open(true)
            .show(ui, |ui| self.object_scan_ui(ui, files));

        egui::CollapsingHeader::new(RichText::new("Presets").strong())
            .default_open(true)
            .show(ui, |ui| self.presets_ui(ui, files));

        ui.separator();
        self.readout_ui(ui);
    }

    /// Play/pause, reset, time scale and frame-rate readout.
    pub fn transport_ui(&mut self, ui: &mut Ui) {
        let emu = &mut self.emu;
        ui.horizontal(|ui| {
            let play = if emu.playing { "⏸ Pause" } else { "▶ Play" };
            if ui.add_sized([80.0, 22.0], egui::Button::new(play)).on_hover_text("Space").clicked() {
                emu.playing = !emu.playing;
            }
            if ui.button("⟲ Reset time").on_hover_text("R (also rewinds the object scan)").clicked() {
                emu.reset_time();
            }
            ui.label(format!("t = {:.2} s", emu.sim_time));
        });
        ui.horizontal(|ui| {
            ui.label("Speed");
            ui.add(
                Slider::new(&mut emu.params.time_scale, 0.05..=10.0)
                    .logarithmic(true)
                    .suffix("×"),
            );
            if ui.small_button("1×").clicked() {
                emu.params.time_scale = 1.0;
            }
        });
    }

    /// Built-in presets, defaults, preset files.
    pub fn presets_ui(&mut self, ui: &mut Ui, files: &mut dyn FileHost) {
        let builtin = presets::builtin();
        ui.horizontal(|ui| {
            self.preset_index = self.preset_index.min(builtin.len() - 1);
            egui::ComboBox::from_id_salt("builtin_presets")
                .width(190.0)
                .selected_text(builtin[self.preset_index].0)
                .show_ui(ui, |ui| {
                    for (i, (name, _)) in builtin.iter().enumerate() {
                        ui.selectable_value(&mut self.preset_index, i, *name);
                    }
                });
            if ui.button("Apply").clicked() {
                let (name, p) = &builtin[self.preset_index];
                self.emu.params = p.clone();
                self.emu.sim_time = 0.0;
                self.set_status(format!("Applied “{name}”"));
            }
        });
        ui.horizontal(|ui| {
            if ui.button("💾 Save…").clicked() {
                let result = presets::to_json(&self.emu.params).and_then(|json| files.save(FileKind::Preset, "laser_preset.json", json.as_bytes()));
                match result {
                    Ok(Some(path)) => self.set_status(format!("Saved {path}")),
                    Ok(None) => {}
                    Err(e) => self.set_status(format!("Save failed: {e}")),
                }
            }
            if ui.button("📂 Load…").clicked() {
                let result = files.open(FileKind::Preset).and_then(|f| {
                    f.map(|f| {
                        let text = String::from_utf8(f.bytes).map_err(|e| format!("{}: {e}", f.location))?;
                        Ok((presets::from_json(&text)?, f.location))
                    })
                    .transpose()
                });
                match result {
                    Ok(Some((p, path))) => {
                        self.emu.params = p;
                        self.set_status(format!("Loaded {path}"));
                    }
                    Ok(None) => {}
                    Err(e) => self.set_status(format!("Load failed: {e}")),
                }
            }
            if ui.button("Reset all").clicked() {
                self.emu.params = LaserParams::default();
                self.set_status("All parameters reset");
            }
        });
        if let Some(msg) = self.status() {
            ui.label(RichText::new(msg).weak());
        }
    }

    /// Scan playback: import, load, save and the playback parameters.
    pub fn object_scan_ui(&mut self, ui: &mut Ui, files: &mut dyn FileHost) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("📂 Import model…").clicked() {
                self.import_model(files);
            }
            let open = self.slicer_open;
            if ui.selectable_label(open, "🔧 Slicer").on_hover_text("Slice settings and 3D preview").clicked() {
                self.slicer_open = !open;
            }
            if ui.button("Load scan…").clicked() {
                self.load_scan(files);
            }
            if ui.add_enabled(self.emu.scan.is_some(), egui::Button::new("Save scan…")).clicked() {
                self.save_scan(files);
            }
        });
        if self.slicer.is_busy() {
            ui.add(egui::Spinner::new());
        }
        let Some(scan) = self.emu.scan.clone() else {
            ui.label(RichText::new("No scan loaded. Import a 3D model (STL, 3MF, OBJ, PLY, glTF, STEP) or load a .lscan file.").weak());
            return;
        };
        scan_summary(ui, &scan);
        ui.checkbox(&mut self.emu.params.scan.enabled, "Drive the line from the scan");

        let row = self.emu.current_row();
        let live = self.live.scan;
        let angle = scan.meta.settings.triangulation_deg;
        scan_params_ui(ui, &mut self.emu.params.scan, live, angle);

        ui.add_space(4.0);
        let rows = scan.meta.rows;
        ui.horizontal(|ui| {
            if ui.small_button("◀").on_hover_text("Previous profile (←)").clicked() {
                self.emu.step_scan(-1);
            }
            if ui.small_button("▶").on_hover_text("Next profile (→)").clicked() {
                self.emu.step_scan(1);
            }
            let mut r = row;
            if ui.add(Slider::new(&mut r, 0..=rows - 1).text("profile")).changed() {
                self.emu.seek_row(r);
            }
        });
        profile_plot(ui, &scan, row, 56.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("↺ Reset").on_hover_text("Reset scan display & playback to defaults").clicked() {
                    self.emu.params.scan = ScanParams::default();
                }
            });
        });
    }

    /// Measurements of the last live frame.
    pub fn readout_ui(&self, ui: &mut Ui) {
        let l = &self.live;
        egui::Grid::new("readout").num_columns(2).show(ui, |ui| {
            ui.label("View");
            ui.label(format!("{:.0} × {:.0} px", l.view_px[0], l.view_px[1]));
            ui.end_row();
            ui.label("Center");
            ui.label(format!("{:.1}, {:.1} px", l.center_px[0], l.center_px[1]));
            ui.end_row();
            ui.label("Length");
            ui.label(format!("{:.0} px", l.length_px));
            ui.end_row();
            ui.label("Frame rate");
            ui.label(format!("{:.0} fps", self.emu.fps));
            ui.end_row();
        });
    }
}

/// Collapsible section with a "Reset" button restoring the section defaults.
pub fn section<T: Default>(ui: &mut Ui, title: &str, open: bool, value: &mut T, body: impl FnOnce(&mut Ui, &mut T)) {
    egui::CollapsingHeader::new(RichText::new(title).strong())
        .default_open(open)
        .show(ui, |ui| {
            body(ui, value);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("↺ Reset").on_hover_text(format!("Reset {title} to defaults")).clicked() {
                        *value = T::default();
                    }
                });
            });
        });
}

/// Two-column label/widget grid with id `id`.
pub fn grid(ui: &mut Ui, id: &str, body: impl FnOnce(&mut Ui)) {
    egui::Grid::new(id).num_columns(2).spacing([10.0, 6.0]).show(ui, body);
}

/// One labelled row of a [`grid`].
pub fn row(ui: &mut Ui, label: &str, widget: impl FnOnce(&mut Ui)) {
    ui.label(label);
    widget(ui);
    ui.end_row();
}

/// Editor of the line parameters; `extent_along` is the view extent along the line in px.
pub fn line_ui(ui: &mut Ui, l: &mut LineParams, extent_along: f32) {
    grid(ui, "line_grid", |ui| {
        row(ui, "Intensity", |ui| {
            ui.add(Slider::new(&mut l.intensity_pct, 0.0..=100.0).suffix(" %"))
                .on_hover_text(format!("Peak pixel value ≈ {:.0} / 255", l.intensity_pct / 100.0 * 255.0));
        });
        row(ui, "Width", |ui| {
            ui.add(
                Slider::new(&mut l.width_px, 0.2..=200.0)
                    .logarithmic(true)
                    .suffix(" px")
                    .max_decimals(2),
            )
            .on_hover_text("Full width at half maximum for Gaussian");
        });
        row(ui, "Length", |ui| {
            ui.vertical(|ui| {
                let range = match l.length_unit {
                    LengthUnit::Pixels => 0.0..=8000.0,
                    LengthUnit::Percent => 0.0..=150.0,
                };
                let suffix = if l.length_unit == LengthUnit::Pixels { " px" } else { " %" };
                ui.add(Slider::new(&mut l.length, range).suffix(suffix));
                ui.horizontal(|ui| {
                    let before = l.length_unit;
                    ui.selectable_value(&mut l.length_unit, LengthUnit::Percent, "% of view");
                    ui.selectable_value(&mut l.length_unit, LengthUnit::Pixels, "pixels");
                    // Keep the visible length when switching units.
                    if before != l.length_unit && extent_along > 0.0 {
                        l.length = match l.length_unit {
                            LengthUnit::Pixels => (l.length / 100.0 * extent_along).round(),
                            LengthUnit::Percent => l.length / extent_along * 100.0,
                        };
                    }
                });
            });
        });
        row(ui, "Position X", |ui| {
            ui.add(Slider::new(&mut l.position_pct[0], 0.0..=100.0).suffix(" %"));
        });
        row(ui, "Position Y", |ui| {
            ui.add(Slider::new(&mut l.position_pct[1], 0.0..=100.0).suffix(" %"));
        });
        row(ui, "", |ui| {
            if ui.small_button("Center").clicked() {
                l.position_pct = [50.0, 50.0];
            }
        });
        row(ui, "Orientation", |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut l.orientation, Orientation::Horizontal, "— Horizontal");
                ui.selectable_value(&mut l.orientation, Orientation::Vertical, "| Vertical");
            });
        });
        row(ui, "Tilt", |ui| {
            ui.add(
                Slider::new(&mut l.tilt_deg, -45.0..=45.0)
                    .suffix("°")
                    .clamping(egui::SliderClamping::Never),
            );
        });
    });
}

/// Editor of the cross-section with its plot; `width` is the line width in px.
pub fn profile_ui(ui: &mut Ui, p: &mut ProfileParams, width: f32) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut p.kind, ProfileKind::Gaussian, "Gaussian");
        ui.selectable_value(&mut p.kind, ProfileKind::TopHat, "Top-hat");
        ui.selectable_value(&mut p.kind, ProfileKind::Hard, "Hard edge");
    });
    grid(ui, "profile_grid", |ui| {
        if p.kind == ProfileKind::TopHat {
            row(ui, "Edge softness", |ui| {
                ui.add(Slider::new(&mut p.edge_softness_px, 0.0..=50.0).suffix(" px"));
            });
        }
        row(ui, "End fade", |ui| {
            ui.add(Slider::new(&mut p.end_softness_px, 0.0..=300.0).suffix(" px"))
                .on_hover_text("Fade-out length at both line ends (0 = hard ends)");
        });
    });
    // Cross-section preview over ±1.5 × the visible width.
    let half = 1.5 * (width + p.edge_softness_px * (p.kind == ProfileKind::TopHat) as u8 as f32).max(1.0);
    plot(ui, -half, half, 0.0, 1.0, |v| model::profile_intensity(p, width, v), "cross-section");
}

/// Editor of the line colour.
pub fn color_ui(ui: &mut Ui, c: &mut ColorParams) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut c.mode, ColorMode::Wavelength, "Wavelength");
        ui.selectable_value(&mut c.mode, ColorMode::Custom, "Custom RGB");
        let [r, g, b] = model::laser_rgb(c);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 18.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 3.0, egui::Rgba::from_rgb(r, g, b));
    });
    match c.mode {
        ColorMode::Wavelength => {
            ui.add(Slider::new(&mut c.wavelength_nm, 380.0..=780.0).suffix(" nm"));
            ui.horizontal(|ui| {
                for nm in WAVELENGTH_PRESETS {
                    let [r, g, b] = model::wavelength_to_rgb(nm);
                    let text = RichText::new(format!("{nm:.0}")).color(egui::Rgba::from_rgb(r, g, b));
                    if ui.selectable_label(c.wavelength_nm == nm, text).clicked() {
                        c.wavelength_nm = nm;
                    }
                }
            });
        }
        ColorMode::Custom => {
            ui.horizontal(|ui| {
                ui.color_edit_button_rgb(&mut c.custom_rgb);
                for (name, rgb) in [("White", [1.0; 3]), ("Red", [1.0, 0.0, 0.0]), ("Green", [0.0, 1.0, 0.0]), ("Blue", [0.0, 0.0, 1.0])] {
                    if ui.small_button(name).clicked() {
                        c.custom_rgb = rgb;
                    }
                }
            });
        }
    }
}

/// Editor of the movement; `offset` is the current offset in px, for the readout.
pub fn motion_ui(ui: &mut Ui, m: &mut MotionParams, offset: f32) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut m.kind, MotionKind::Static, "Static");
        ui.selectable_value(&mut m.kind, MotionKind::Sweep, "Sweep");
        ui.selectable_value(&mut m.kind, MotionKind::Sine, "Oscillate");
    });
    grid(ui, "motion_grid", |ui| match m.kind {
        MotionKind::Static => {}
        MotionKind::Sweep => {
            row(ui, "Speed", |ui| {
                ui.add(Slider::new(&mut m.speed_px_s, -3000.0..=3000.0).suffix(" px/s"))
                    .on_hover_text("Negative values reverse the direction");
            });
            row(ui, "Span", |ui| {
                ui.add(Slider::new(&mut m.span_pct, 0.0..=100.0).suffix(" %"))
                    .on_hover_text("Travel range, percent of the view across the line, centered on the position");
            });
            row(ui, "At end", |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut m.sweep_mode, SweepMode::Bounce, "Bounce");
                    ui.selectable_value(&mut m.sweep_mode, SweepMode::Wrap, "Wrap");
                });
            });
        }
        MotionKind::Sine => {
            row(ui, "Amplitude", |ui| {
                ui.add(Slider::new(&mut m.amplitude_px, 0.0..=1000.0).suffix(" px"));
            });
            row(ui, "Frequency", |ui| {
                ui.add(Slider::new(&mut m.frequency_hz, 0.01..=20.0).logarithmic(true).suffix(" Hz"));
            });
            row(ui, "Phase", |ui| {
                ui.add(Slider::new(&mut m.phase_deg, -180.0..=180.0).suffix("°"));
            });
        }
    });
    if m.kind != MotionKind::Static {
        ui.label(RichText::new(format!("Current offset: {offset:+.1} px")).weak());
    }
}

/// Editor of the deformation with its plot over `length_px` at time `t` s.
pub fn deform_ui(ui: &mut Ui, d: &mut DeformParams, length_px: f32, t: f64) {
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut d.kind, DeformKind::None, "None");
        ui.selectable_value(&mut d.kind, DeformKind::Step, "Step");
        ui.selectable_value(&mut d.kind, DeformKind::Bump, "Bump");
        ui.selectable_value(&mut d.kind, DeformKind::Box, "Box");
        ui.selectable_value(&mut d.kind, DeformKind::Sine, "Wave");
    });
    if d.kind == DeformKind::None {
        return;
    }
    grid(ui, "deform_grid", |ui| {
        match d.kind {
            DeformKind::Sine => {
                row(ui, "Amplitude", |ui| {
                    ui.add(px(&mut d.amplitude_px, 0.0..=300.0));
                });
                row(ui, "Wavelength", |ui| {
                    ui.add(px(&mut d.wavelength_px, 4.0..=4000.0).logarithmic(true));
                });
            }
            _ => {
                row(ui, "Position", |ui| {
                    ui.add(px(&mut d.position_px, -2000.0..=2000.0))
                        .on_hover_text("Along the line, relative to its center");
                });
                row(ui, "Height", |ui| {
                    ui.add(px(&mut d.height_px, -500.0..=500.0))
                        .on_hover_text("Displacement across the line; sign sets the direction");
                });
                if matches!(d.kind, DeformKind::Bump | DeformKind::Box) {
                    row(ui, "Width", |ui| {
                        ui.add(px(&mut d.width_px, 1.0..=3000.0).logarithmic(true));
                    });
                }
                if d.kind == DeformKind::Box {
                    row(ui, "Flank", |ui| {
                        ui.add(px(&mut d.slope_px, 0.0..=1000.0));
                    });
                }
            }
        }
        row(ui, "Travel", |ui| {
            ui.add(Slider::new(&mut d.scroll_px_s, -2000.0..=2000.0).suffix(" px/s"))
                .on_hover_text("Moves the feature along the line, like an object passing through");
        });
    });
    let half = 0.5 * length_px.max(10.0);
    let (lo, hi) = (0..=200)
        .map(|i| model::deform_offset(d, -half + i as f32 * half / 100.0, t))
        .fold((0.0f32, 0.0f32), |(lo, hi), v| (lo.min(v), hi.max(v)));
    let pad = ((hi - lo) * 0.1).max(1.0);
    plot(ui, -half, half, lo - pad, hi + pad, |u| model::deform_offset(d, u, t), "shape along the line");
}

/// Pixel slider that still accepts typed values outside its range.
fn px(v: &mut f32, range: std::ops::RangeInclusive<f32>) -> Slider<'_> {
    Slider::new(v, range).suffix(" px").clamping(egui::SliderClamping::Never)
}

/// Scale, height gain and playback of the scan. `live` is the mapping used in the last frame.
pub fn scan_params_ui(ui: &mut Ui, s: &mut ScanParams, live: Option<DisplayMapping>, angle_deg: f32) {
    grid(ui, "scan_grid", |ui| {
        row(ui, "Scale", |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut s.scale, ScanScale::FitLine, "Fit line length")
                    .on_hover_text("Stretch the scan width over the line length set above");
                ui.selectable_value(&mut s.scale, ScanScale::Fixed, "Fixed");
            });
        });
        if s.scale == ScanScale::Fixed {
            row(ui, "", |ui| {
                ui.add(Slider::new(&mut s.px_per_mm, 0.05..=200.0).logarithmic(true).suffix(" px/mm"))
                    .on_hover_text("Screen pixels per mm along the line; sets the line length");
            });
        }
        row(ui, "Height gain", |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut s.height_gain, HeightGain::MatchScale, "1:1")
                        .on_hover_text("Same px/mm as along the line: the line shows the true-scale profile");
                    ui.selectable_value(&mut s.height_gain, HeightGain::FromAngle, format!("sin {angle_deg:.0}°"))
                        .on_hover_text("px/mm · sin(triangulation angle of the scan): what a camera at that angle sees");
                    ui.selectable_value(&mut s.height_gain, HeightGain::Manual, "Manual");
                });
                match s.height_gain {
                    HeightGain::Manual => {
                        ui.add(Slider::new(&mut s.height_px_per_mm, 0.0..=200.0).logarithmic(true).suffix(" px/mm"))
                            .on_hover_text("Line displacement per mm of object height");
                    }
                    HeightGain::FromAngle if angle_deg < 1.0 => {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            "The scan was sliced at a triangulation angle of 0°, so the height gain is zero and the line stays flat.",
                        );
                    }
                    _ => {}
                }
                ui.checkbox(&mut s.invert_height, "Invert (higher = down)");
            });
        });
        if let Some(m) = live {
            row(ui, "", |ui| {
                ui.label(
                    RichText::new(format!(
                        "{:.3} px/mm ({:.4} mm/px) · height {:.3} px/mm",
                        m.px_per_mm,
                        1.0 / m.px_per_mm.max(1e-9),
                        m.height_px_per_mm
                    ))
                    .weak(),
                );
            });
        }
        row(ui, "Rate", |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut s.rate, RateMode::PerFrame, "1 / frame")
                    .on_hover_text("One profile per displayed frame, for a camera synchronised to the display");
                ui.selectable_value(&mut s.rate, RateMode::PerSecond, "Fixed rate");
            });
        });
        if s.rate == RateMode::PerSecond {
            row(ui, "", |ui| {
                ui.add(Slider::new(&mut s.profiles_per_s, 0.1..=1000.0).logarithmic(true).suffix(" profiles/s"))
                    .on_hover_text("Scaled by the simulation speed");
            });
        }
        row(ui, "At end", |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut s.end, EndMode::Loop, "Loop");
                ui.selectable_value(&mut s.end, EndMode::Bounce, "Bounce");
                ui.selectable_value(&mut s.end, EndMode::Once, "Once");
            });
        });
    });
}

/// Editor of the noise.
pub fn noise_ui(ui: &mut Ui, n: &mut NoiseParams) {
    grid(ui, "noise_grid", |ui| {
        row(ui, "Ambient light", |ui| {
            ui.add(Slider::new(&mut n.ambient_pct, 0.0..=50.0).suffix(" %"));
        });
        row(ui, "Speckle", |ui| {
            ui.add(Slider::new(&mut n.speckle_pct, 0.0..=100.0).suffix(" %"))
                .on_hover_text("Speckle contrast on the laser line");
        });
        if n.speckle_pct > 0.0 {
            row(ui, "Grain size", |ui| {
                ui.add(Slider::new(&mut n.grain_px, 0.5..=20.0).logarithmic(true).suffix(" px"));
            });
        }
        row(ui, "Sensor noise σ", |ui| {
            ui.add(Slider::new(&mut n.noise_pct, 0.0..=20.0).suffix(" %"));
        });
        row(ui, "", |ui| {
            ui.checkbox(&mut n.animated, "Animated").on_hover_text("Off = frozen pattern");
        });
    });
}

/// Small line plot of `f` over [x0, x1] with y range [y0, y1].
pub fn plot(ui: &mut Ui, x0: f32, x1: f32, y0: f32, y1: f32, f: impl Fn(f32) -> f32, caption: &str) {
    let width = ui.available_width().min(340.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 56.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    painter.rect_filled(rect, 3.0, visuals.extreme_bg_color);

    let to_screen = |x: f32, y: f32| {
        egui::pos2(
            egui::remap(x, x0..=x1, rect.left()..=rect.right()),
            egui::remap(y, y0..=y1, rect.bottom() - 3.0..=rect.top() + 3.0),
        )
    };
    if y0 < 0.0 && y1 > 0.0 {
        painter.line_segment([to_screen(x0, 0.0), to_screen(x1, 0.0)], Stroke::new(1.0, visuals.weak_text_color()));
    }
    let n = (rect.width() as usize).max(2);
    let points = (0..=n)
        .map(|i| {
            let x = x0 + (x1 - x0) * i as f32 / n as f32;
            to_screen(x, f(x))
        })
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(1.5, Color32::from_rgb(255, 80, 60))));
    painter.text(rect.left_top() + egui::vec2(4.0, 2.0), egui::Align2::LEFT_TOP, caption, egui::FontId::proportional(10.0), visuals.weak_text_color());
}
