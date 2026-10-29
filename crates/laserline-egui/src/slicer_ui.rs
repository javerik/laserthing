//! Object slicer UI: model import, slice settings, slicing progress and the previews
//! (3D view, range image, current profile).

use crate::files::FileHost;
use crate::laser_ui::{LaserUi, grid, row};
use crate::callbacks::MeshViewCallback;
use egui::{self, Color32, RichText, Slider, Stroke, Ui};
use laserline_core::camera::OrbitCamera;
use laserline_core::glam::Vec3;
use laserline_core::scan::{self, Axis, CameraSide, ModelUnit, Scan, SliceSettings, UpAxis};
use laserline_core::slicer::Slicer;
use laserline_core::viz;
use std::sync::Arc;

const VISIBLE: Color32 = rgb(viz::VISIBLE);
const OCCLUDED: Color32 = rgb(viz::OCCLUDED);

const fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Screen position of `p` inside `rect`, or `None` behind the camera.
pub fn project(camera: &OrbitCamera, rect: egui::Rect, p: Vec3) -> Option<egui::Pos2> {
    let [x, y] = camera.project(rect.aspect_ratio(), p)?;
    Some(egui::pos2(rect.left() + x * rect.width(), rect.top() + y * rect.height()))
}

/// Drag: orbit; secondary drag or shift-drag: pan; scroll / pinch: zoom.
pub fn handle_camera_input(camera: &mut OrbitCamera, ui: &egui::Ui, response: &egui::Response) {
    let drag = response.drag_delta();
    let shift = ui.input(|i| i.modifiers.shift);
    if response.dragged_by(egui::PointerButton::Secondary) || (response.dragged_by(egui::PointerButton::Primary) && shift) {
        camera.pan(drag.x, drag.y);
    } else if response.dragged_by(egui::PointerButton::Primary) {
        camera.orbit(drag.x, drag.y);
    }
    if response.hovered() {
        let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        camera.zoom(scroll, zoom);
    }
}

impl LaserUi {
    /// Settings on the left, previews on the right; fills `ui`.
    pub fn slicer_panels(&mut self, ui: &mut Ui, files: &mut dyn FileHost) {
        let scan = self.emu.scan.clone();
        self.slicer.update_preview(scan.as_ref());
        egui::SidePanel::left("slicer_settings")
            .resizable(true)
            .default_width(330.0)
            .min_width(280.0)
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.slicer_settings_ui(ui, files));
            });
        egui::CentralPanel::default().show_inside(ui, |ui| self.slicer_preview_ui(ui, scan));
    }

    fn slicer_settings_ui(&mut self, ui: &mut Ui, files: &mut dyn FileHost) {
        ui.spacing_mut().slider_width = 150.0;
        ui.horizontal(|ui| {
            ui.heading("Object");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("📂 Import model…").clicked() {
                    self.import_model(files);
                }
            });
        });
        let sl = &mut self.slicer;
        match &sl.model {
            Some(m) => {
                ui.label(RichText::new(&m.name).strong()).on_hover_text(&m.location);
                let (lo, hi) = m.bounds;
                ui.label(
                    RichText::new(format!(
                        "{} triangles · {:.4} × {:.4} × {:.4} model units",
                        m.mesh.triangles.len(),
                        hi[0] - lo[0],
                        hi[1] - lo[1],
                        hi[2] - lo[2]
                    ))
                    .weak(),
                );
            }
            None if self.emu.scan.is_some() => {
                ui.label(RichText::new("Playing a loaded scan; its settings are shown below. Import the model to re-slice.").weak());
            }
            None => {
                ui.label(RichText::new("No model loaded. Import a 3D model (STL, 3MF, OBJ, PLY, glTF, STEP) to slice it.").weak());
            }
        }
        ui.separator();

        // Settings of a scan loaded without its model can't be applied, only shown.
        let editable = sl.model.is_some() || self.emu.scan.is_none();
        ui.add_enabled_ui(editable, |ui| slice_settings_ui(ui, &mut self.slicer));
        ui.separator();
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.slicer.auto, "Auto re-slice");
            ui.add_enabled_ui(self.slicer.model.is_some(), |ui| {
                if ui.button("⟳ Slice now").clicked() {
                    self.slice_now();
                }
            });
        });
        let sl = &self.slicer;
        if let Some(f) = self.worker.progress().filter(|_| sl.is_busy()) {
            ui.add(egui::ProgressBar::new(f).text("Slicing…").animate(true));
        } else if sl.is_stale() {
            ui.label(RichText::new("Settings changed since the last slice").weak());
        } else if let Some(d) = sl.last_duration_s
            && sl.model.is_some()
        {
            ui.label(RichText::new(format!("Sliced in {d:.2} s")).weak());
        }
        if let Some(e) = &sl.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        if let Some(scan) = &self.emu.scan {
            ui.separator();
            scan_summary(ui, scan);
            ui.horizontal(|ui| {
                if ui.button("💾 Save scan…").clicked() {
                    self.save_scan(files);
                }
                if ui.button("📂 Load scan…").clicked() {
                    self.load_scan(files);
                }
            });
        }
    }

    fn slicer_preview_ui(&mut self, ui: &mut Ui, scan: Option<Arc<Scan>>) {
        let row = self.emu.current_row();
        let avail = ui.available_size();
        let bottom = if scan.is_some() { 300.0 } else { 0.0 };
        let view_h = (avail.y - bottom).max(160.0);
        let (rect, response) = ui.allocate_exact_size(egui::vec2(avail.x, view_h), egui::Sense::click_and_drag());
        self.view_3d(ui, rect, &response, scan.as_deref(), row);

        let Some(scan) = scan else { return };
        let rows = scan.meta.rows;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Range image").strong());
            ui.label(RichText::new("x: travel · y: along line · magenta: occluded · click to pick a profile").weak().small());
        });
        if let Some(r) = self.range_image_ui(ui, &scan, row, 130.0) {
            self.emu.seek_row(r);
        }
        ui.add_space(4.0);
        profile_plot(ui, &scan, row, 100.0);
        ui.horizontal(|ui| {
            if ui.button("◀").on_hover_text("Previous profile (←)").clicked() {
                self.emu.step_scan(-1);
            }
            if ui.button("▶").on_hover_text("Next profile (→)").clicked() {
                self.emu.step_scan(1);
            }
            let mut r = row;
            ui.spacing_mut().slider_width = (ui.available_width() - 220.0).max(80.0);
            if ui.add(Slider::new(&mut r, 0..=rows - 1).text("profile")).changed() {
                self.emu.seek_row(r);
            }
            ui.label(format!("v = {:.2} mm", scan.meta.v0_mm + row as f32 * scan.meta.dv_mm));
        });
    }

    fn view_3d(&mut self, ui: &mut Ui, rect: egui::Rect, response: &egui::Response, scan: Option<&Scan>, row: usize) {
        let painter = ui.painter_at(rect);
        let reset = response.double_clicked();
        if let Some(camera) = &mut self.slicer.camera {
            handle_camera_input(camera, ui, response);
        }
        let Some(view) = self.slicer.view_3d(scan, row, reset) else {
            painter.rect_filled(rect, 0.0, Color32::from_gray(18));
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "Import a 3D model to start", egui::FontId::proportional(18.0), Color32::from_gray(140));
            return;
        };
        let ppp = ui.ctx().pixels_per_point();
        painter.add(egui_wgpu::Callback::new_paint_callback(
            rect,
            MeshViewCallback {
                mesh: view.mesh,
                overlay: Arc::new(view.overlay.overlay),
                uniforms: view.camera.uniforms(rect.aspect_ratio(), view.z_max),
                size_px: [(rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32],
                srgb_target: self.srgb_target(),
            },
        ));
        for label in view.overlay.labels {
            if let Some(p) = project(&view.camera, rect, label.pos) {
                painter.text(p, egui::Align2::LEFT_BOTTOM, label.text, egui::FontId::proportional(13.0), rgb(label.color));
            }
        }
        painter.text(
            rect.left_bottom() + egui::vec2(8.0, -6.0),
            egui::Align2::LEFT_BOTTOM,
            "drag: orbit · right/shift-drag: pan · scroll: zoom · double-click: reset",
            egui::FontId::proportional(11.0),
            Color32::from_gray(120),
        );
    }

    /// Draws the range image; returns the profile under the pointer while clicked/dragged.
    fn range_image_ui(&mut self, ui: &mut Ui, scan: &Arc<Scan>, row: usize, height: f32) -> Option<usize> {
        if !self.range_image.as_ref().is_some_and(|(_, s)| Arc::ptr_eq(s, scan)) {
            let img = viz::range_image(scan);
            let img = egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.pixels);
            let tex = ui.ctx().load_texture("range_image", img, egui::TextureOptions::LINEAR);
            self.range_image = Some((tex, scan.clone()));
        }
        let (tex, _) = self.range_image.as_ref().unwrap();
        let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.image(tex.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        let rows = scan.meta.rows;
        let x = rect.left() + (row as f32 + 0.5) / rows as f32 * rect.width();
        painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], Stroke::new(1.5, VISIBLE));
        let pointer = response.interact_pointer_pos()?;
        (response.is_pointer_button_down_on() || response.clicked())
            .then(|| (((pointer.x - rect.left()) / rect.width() * rows as f32) as usize).min(rows - 1))
    }
}

/// Orientation, scale, resolution and sensor settings of the slicer.
pub fn slice_settings_ui(ui: &mut Ui, slicer: &mut Slicer) {
    let preview = slicer.preview().cloned();
    let has_model = slicer.model.is_some();
    let s = &mut slicer.settings;
    egui::CollapsingHeader::new(RichText::new("Orientation").strong()).default_open(true).show(ui, |ui| {
        grid(ui, "slice_orient", |ui| {
            row(ui, "Up axis", |ui| {
                egui::ComboBox::from_id_salt("up_axis").selected_text(up_label(s.up_axis)).show_ui(ui, |ui| {
                    for a in [UpAxis::PosX, UpAxis::NegX, UpAxis::PosY, UpAxis::NegY, UpAxis::PosZ, UpAxis::NegZ] {
                        ui.selectable_value(&mut s.up_axis, a, up_label(a));
                    }
                });
                ui.label("").on_hover_text("Model axis pointing towards the sensor");
            });
            let up = s.up_axis.index_sign().0;
            row(ui, "Travel axis", |ui| {
                ui.horizontal(|ui| {
                    for (i, a) in [Axis::X, Axis::Y, Axis::Z].into_iter().enumerate() {
                        ui.add_enabled_ui(i != up, |ui| ui.selectable_value(&mut s.travel_axis, a, axis_label(i)));
                    }
                });
            });
            // Keep the travel axis valid after the up axis changed.
            s.travel_axis = [Axis::X, Axis::Y, Axis::Z][s.travel_axis_index()];
            let along = 3 - up - s.travel_axis_index();
            row(ui, "Along line", |ui| {
                ui.checkbox(&mut s.flip_along, format!("{} (mirrored)", axis_label(along)))
                    .on_hover_text("The remaining axis runs along the laser line; tick to mirror it");
            });
            row(ui, "Rotation", |ui| {
                ui.horizontal(|ui| {
                    ui.add(Slider::new(&mut s.yaw_deg, -180.0..=180.0).suffix("°"))
                        .on_hover_text("Rotates the object on the conveyor, about the up axis");
                    if ui.small_button("0").clicked() {
                        s.yaw_deg = 0.0;
                    }
                });
            });
        });
    });

    egui::CollapsingHeader::new(RichText::new("Scale").strong()).default_open(true).show(ui, |ui| {
        grid(ui, "slice_scale", |ui| {
            row(ui, "Model unit", |ui| {
                egui::ComboBox::from_id_salt("model_unit").selected_text(unit_label(s.unit)).show_ui(ui, |ui| {
                    for u in [ModelUnit::Mm, ModelUnit::Cm, ModelUnit::M, ModelUnit::Inch] {
                        ui.selectable_value(&mut s.unit, u, unit_label(u));
                    }
                });
            });
            row(ui, "Scale", |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut s.scale).speed(0.01).range(0.001..=1000.0).suffix("×"));
                    if ui.small_button("1×").clicked() {
                        s.scale = 1.0;
                    }
                });
            });
            if let Some(p) = &preview
                && has_model
            {
                let (lo, hi) = p.bounds;
                let d = hi - lo;
                row(ui, "Size", |ui| {
                    ui.label(format!("{:.1} × {:.1} × {:.1} mm", d.x, d.y, d.z))
                        .on_hover_text("Along line × travel × height");
                });
            }
        });
    });

    egui::CollapsingHeader::new(RichText::new("Resolution").strong()).default_open(true).show(ui, |ui| {
        grid(ui, "slice_res", |ui| {
            row(ui, "Along line", |ui| {
                ui.add(mm_slider(&mut s.lateral_res_mm, 0.005..=10.0).logarithmic(true))
                    .on_hover_text("Sample spacing along the laser line");
            });
            row(ui, "Travel step", |ui| {
                ui.add(mm_slider(&mut s.travel_step_mm, 0.005..=50.0).logarithmic(true))
                    .on_hover_text("Object movement between two profiles");
            });
            row(ui, "Side margin", |ui| {
                ui.add(mm_slider(&mut s.margin_lateral_mm, 0.0..=500.0));
            });
            row(ui, "Lead in/out", |ui| {
                ui.add(mm_slider(&mut s.margin_travel_mm, 0.0..=500.0))
                    .on_hover_text("Empty travel before and after the object");
            });
        });
        if let Some(p) = preview.as_ref().filter(|_| has_model) {
            match scan::grid_size(p.bounds.0.into(), p.bounds.1.into(), s) {
                Ok((c, r)) => {
                    let mb = (c * r * 5) as f32 / 1e6;
                    ui.label(RichText::new(format!("{c} samples × {r} profiles · {mb:.1} MB")).weak());
                }
                Err(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
            }
        }
    });

    egui::CollapsingHeader::new(RichText::new("Sensor").strong()).default_open(true).show(ui, |ui| {
        grid(ui, "slice_sensor", |ui| {
            row(ui, "Conveyor", |ui| {
                ui.checkbox(&mut s.base_plane, "Plane at z = 0")
                    .on_hover_text("Keep the line visible next to the object, as on a belt");
            });
            row(ui, "Triangulation", |ui| {
                ui.add(Slider::new(&mut s.triangulation_deg, 0.0..=80.0).suffix("°"))
                    .on_hover_text("Angle between camera axis and laser plane; also sets the default height gain");
            });
            row(ui, "Occlusion", |ui| {
                ui.checkbox(&mut s.occlusion, "Hide what the camera can't see");
            });
            row(ui, "Camera side", |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut s.camera_side, CameraSide::Ahead, "Ahead (+travel)");
                    ui.selectable_value(&mut s.camera_side, CameraSide::Behind, "Behind");
                });
            });
        });
    });
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("↺ Reset").on_hover_text("Reset slice settings to defaults").clicked() {
                *s = SliceSettings::default();
            }
        });
    });
}

/// Height along the line for one profile: red where the camera sees it, magenta where occluded.
pub fn profile_plot(ui: &mut Ui, scan: &Scan, row: usize, height: f32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    painter.rect_filled(rect, 3.0, visuals.extreme_bg_color);
    let m = &scan.meta;
    let z_top = (m.z_max_mm * 1.1).max(1e-3);
    let to_screen = |i: f32, z: f32| {
        egui::pos2(
            egui::remap(i, 0.0..=(m.cols.max(2) - 1) as f32, rect.left() + 2.0..=rect.right() - 2.0),
            egui::remap(z, 0.0..=z_top, rect.bottom() - 4.0..=rect.top() + 14.0),
        )
    };
    for run in viz::profile_runs(scan, row) {
        let color = if run.visible { VISIBLE } else { OCCLUDED };
        let points = run.points.iter().map(|&[i, z]| to_screen(i, z)).collect();
        painter.add(egui::Shape::line(points, Stroke::new(1.5, color)));
    }
    painter.text(
        rect.left_top() + egui::vec2(4.0, 2.0),
        egui::Align2::LEFT_TOP,
        format!("profile {row} · width {:.1} mm · max {:.2} mm", scan.width_mm(), m.z_max_mm),
        egui::FontId::proportional(10.0),
        visuals.weak_text_color(),
    );
}

/// Source, grid size and extent of `scan`.
pub fn scan_summary(ui: &mut Ui, scan: &Scan) {
    let m = &scan.meta;
    ui.label(RichText::new(if m.source.is_empty() { "Scan" } else { &m.source }).strong());
    ui.label(
        RichText::new(format!(
            "{} profiles × {} samples · {:.1} × {:.1} mm · max height {:.2} mm",
            m.rows,
            m.cols,
            scan.width_mm(),
            scan.length_mm(),
            m.z_max_mm
        ))
        .weak(),
    );
}

fn mm_slider(v: &mut f32, range: std::ops::RangeInclusive<f32>) -> Slider<'_> {
    Slider::new(v, range).suffix(" mm").clamping(egui::SliderClamping::Never)
}

fn axis_label(i: usize) -> &'static str {
    ["X", "Y", "Z"][i]
}

fn up_label(a: UpAxis) -> &'static str {
    match a {
        UpAxis::PosX => "+X",
        UpAxis::NegX => "−X",
        UpAxis::PosY => "+Y",
        UpAxis::NegY => "−Y",
        UpAxis::PosZ => "+Z",
        UpAxis::NegZ => "−Z",
    }
}

pub(crate) fn unit_label(u: ModelUnit) -> &'static str {
    match u {
        ModelUnit::Mm => "mm",
        ModelUnit::Cm => "cm",
        ModelUnit::M => "m",
        ModelUnit::Inch => "inch",
    }
}

