//! Application shell: the main window, the dockable / detachable live view and control
//! panel, the slicer window, keyboard shortcuts and persistence.

use crate::native_files::{self, NativeFiles};
use eframe::egui::{self, Color32, ViewportBuilder, ViewportCommand, ViewportId};
use laserline_egui::core::params::LaserParams;
use laserline_egui::core::scan::SliceSettings;
use laserline_egui::{Action, DockState, LaserUi, Layout, ViewPlacement, show_window};
use std::time::{Duration, Instant};

pub struct LaserApp {
    pub ui: LaserUi,
    pub layout: Layout,
    files: NativeFiles,
    last_frame: Instant,
    hidden_at: Option<Instant>,
}

const KEY_PARAMS: &str = "params";
const KEY_LAYOUT: &str = "layout";
// Before `layout`: the controls and view placement were stored separately.
const KEY_DOCK: &str = "dock";
const KEY_VIEW: &str = "view_placement";
const KEY_SLICE: &str = "slice_settings";
const KEY_SLICE_AUTO: &str = "slice_auto";
const CONTROLS_VIEWPORT: &str = "laser_controls";
const SLICER_VIEWPORT: &str = "object_slicer";
const LIVE_VIEWPORT: &str = "laser_live_view";

impl LaserApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        let rs = cc
            .wgpu_render_state
            .as_ref()
            .ok_or("wgpu renderer not available")?;
        LaserUi::install_gpu(rs);

        let (params, layout, slice, auto) = match cc.storage {
            Some(s) => (
                eframe::get_value(s, KEY_PARAMS).unwrap_or_default(),
                eframe::get_value::<Layout>(s, KEY_LAYOUT).unwrap_or_else(|| Layout {
                    controls: eframe::get_value(s, KEY_DOCK).unwrap_or_default(),
                    view: eframe::get_value(s, KEY_VIEW).unwrap_or_default(),
                    ..Default::default()
                }),
                eframe::get_value(s, KEY_SLICE).unwrap_or_default(),
                eframe::get_value(s, KEY_SLICE_AUTO).unwrap_or(true),
            ),
            None => (LaserParams::default(), Layout::default(), SliceSettings::default(), true),
        };

        Ok(Self {
            ui: LaserUi::new(params, slice, auto, rs.target_format.is_srgb()),
            layout: layout.restored(),
            files: NativeFiles,
            last_frame: Instant::now(),
            hidden_at: None,
        })
    }

    /// Opens a 3D model (into the slicer) or a `.lscan` scan given on the command line.
    /// Opening reports for itself (e.g. the model's triangles).
    pub fn open_path(&mut self, path: &std::path::Path) {
        if let Err(e) = native_files::read(path).and_then(|f| self.ui.open_file(f)) {
            self.ui.set_status(e);
        }
    }

    /// Runs `action`, sending the fullscreen command it asks for.
    pub fn apply(&mut self, ctx: &egui::Context, action: Action) {
        let was_hidden = self.layout.controls == DockState::Hidden;
        // Before applying: detaching the view leaves fullscreen in the window it was in.
        let live = self.live_viewport();
        if let Some(on) = self.ui.apply(action, &mut self.layout) {
            ctx.send_viewport_cmd_to(live, ViewportCommand::Fullscreen(on));
        }
        match (was_hidden, self.layout.controls == DockState::Hidden) {
            (false, true) => self.hidden_at = Some(Instant::now()),
            (true, false) => self.hidden_at = None,
            _ => {}
        }
    }

    /// The window showing the live view.
    fn live_viewport(&self) -> ViewportId {
        match self.layout.view {
            ViewPlacement::Docked => ViewportId::ROOT,
            ViewPlacement::Detached => ViewportId::from_hash_of(LIVE_VIEWPORT),
        }
    }

    /// `in_live_window`: the keys were pressed in the window showing the live view.
    fn handle_shortcuts(&mut self, ctx: &egui::Context, in_live_window: bool) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let actions: Vec<Action> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } => Action::from_key(*key, *modifiers),
                    _ => None,
                })
                .collect()
        });
        for action in actions {
            // Only from the live window, so the panel can't hide itself away from its own window.
            if action != Action::ToggleControlsHidden || in_live_window {
                self.apply(ctx, action);
            }
        }
    }

    fn show_detached(&mut self, ctx: &egui::Context) {
        let builder = ViewportBuilder::default()
            .with_title("Laser Controls")
            .with_app_id(format!("{}-controls", crate::APP_ID))
            .with_inner_size([400.0, 760.0])
            .with_min_inner_size([300.0, 300.0]);
        let id = ViewportId::from_hash_of(CONTROLS_VIEWPORT);
        let frame = egui::Frame::central_panel(&ctx.style());
        let open = show_window(ctx, id, builder, frame, |ui| {
            self.handle_shortcuts(ui.ctx(), false);
            self.controls_scroll(ui);
        });
        if !open {
            // Closing the controls window docks the panel instead of hiding it for good.
            self.layout.controls = DockState::Docked;
        }
    }

    fn show_live_window(&mut self, ctx: &egui::Context) {
        let builder = ViewportBuilder::default()
            .with_title("Laser Line — Live View")
            .with_app_id(format!("{}-live", crate::APP_ID))
            .with_inner_size([1280.0, 720.0])
            .with_min_inner_size([320.0, 240.0]);
        let id = ViewportId::from_hash_of(LIVE_VIEWPORT);
        let open = show_window(ctx, id, builder, egui::Frame::NONE.fill(Color32::BLACK), |ui| {
            self.handle_shortcuts(ui.ctx(), true);
            self.live_view(ui);
        });
        if !open {
            // Closing the live view window docks the view into the main window.
            self.layout.view = ViewPlacement::Docked;
        }
    }

    fn show_slicer(&mut self, ctx: &egui::Context) {
        let builder = ViewportBuilder::default()
            .with_title("Object Slicer")
            .with_app_id(format!("{}-slicer", crate::APP_ID))
            .with_inner_size([1200.0, 820.0])
            .with_min_inner_size([640.0, 480.0]);
        let id = ViewportId::from_hash_of(SLICER_VIEWPORT);
        let open = show_window(ctx, id, builder, egui::Frame::NONE, |ui| self.ui.slicer_panels(ui, &mut self.files));
        self.ui.slicer_open &= open;
    }

    fn controls_scroll(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.header_ui(ui);
                ui.separator();
                self.ui.controls_ui(ui, &mut self.files);
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!("Keys: {} · double-click view = fullscreen", Action::help()))
                        .small()
                        .weak(),
                );
            });
    }

    fn header_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Laser Line");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Hide").on_hover_text("Hide controls (H in live view)").clicked() {
                    self.apply(ui.ctx(), Action::ToggleControlsHidden);
                }
                let fs = if self.layout.fullscreen { "Windowed" } else { "Fullscreen" };
                if ui.button(fs).on_hover_text("Toggle live view fullscreen (F11)").clicked() {
                    self.apply(ui.ctx(), Action::ToggleFullscreen);
                }
            });
        });
        ui.horizontal(|ui| {
            let (label, tip) = match self.layout.view {
                ViewPlacement::Docked => ("⏏ Detach view", "Move the live view into its own window (V)"),
                ViewPlacement::Detached => ("Dock view", "Dock the live view into the main window (V)"),
            };
            if ui.button(label).on_hover_text(tip).clicked() {
                self.apply(ui.ctx(), Action::ToggleViewDetached);
            }
            let (label, tip) = match self.layout.controls {
                DockState::Docked => ("⏏ Detach controls", "Move the controls into their own window (D)"),
                _ => ("Dock controls", "Dock the controls into the main window (D)"),
            };
            if ui.button(label).on_hover_text(tip).clicked() {
                self.apply(ui.ctx(), Action::ToggleControlsDetached);
            }
        });
    }

    /// Main window content when both the live view and the controls are elsewhere.
    fn placeholder(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() / 2.0 - 40.0).max(0.0));
            ui.label("The live view and the controls are in their own windows.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Dock view").clicked() {
                    self.apply(ui.ctx(), Action::ToggleViewDetached);
                }
                let controls = if self.layout.controls == DockState::Hidden { "Show controls" } else { "Dock controls" };
                if ui.button(controls).clicked() {
                    self.layout.controls = DockState::Docked;
                    self.hidden_at = None;
                }
            });
        });
    }

    fn live_view(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let response = ui.allocate_rect(rect, egui::Sense::click());
        self.ui.live_view(ui, rect);
        // The state of whichever window shows the view.
        self.layout.fullscreen = ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));

        if response.double_clicked() {
            self.apply(ui.ctx(), Action::ToggleFullscreen);
        }
        if self.layout.fullscreen && self.layout.view_alone() && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        }

        // Brief hint after hiding the controls, then a clean picture.
        if let Some(t) = self.hidden_at {
            let age = t.elapsed();
            if age < Duration::from_secs(3) {
                ui.painter().text(
                    rect.center_bottom() - egui::vec2(0.0, 24.0),
                    egui::Align2::CENTER_BOTTOM,
                    "Controls hidden — press H to show",
                    egui::FontId::proportional(16.0),
                    Color32::from_gray(160),
                );
            } else {
                self.hidden_at = None;
            }
        }
    }
}

impl eframe::App for LaserApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f64();
        self.last_frame = now;

        self.handle_shortcuts(ctx, self.layout.view == ViewPlacement::Docked);
        self.ui.begin_frame(ctx, dt);
        if self.ui.slicer_open {
            self.show_slicer(ctx);
        }
        if self.layout.view == ViewPlacement::Detached {
            self.show_live_window(ctx);
        }

        let view_docked = self.layout.view == ViewPlacement::Docked;
        match self.layout.controls {
            DockState::Docked if view_docked => {
                egui::SidePanel::right("controls_panel")
                    .resizable(true)
                    .default_width(360.0)
                    .min_width(280.0)
                    .show(ctx, |ui| self.controls_scroll(ui));
            }
            // The live view is elsewhere: the controls get the whole main window.
            DockState::Docked => {
                egui::CentralPanel::default().show(ctx, |ui| self.controls_scroll(ui));
            }
            DockState::Detached => self.show_detached(ctx),
            DockState::Hidden => {}
        }

        if view_docked {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(Color32::BLACK))
                .show(ctx, |ui| self.live_view(ui));
        } else if self.layout.controls != DockState::Docked {
            egui::CentralPanel::default().show(ctx, |ui| self.placeholder(ui));
        }

        // Live emulator: animate continuously (vsync-limited).
        ctx.request_repaint();
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, KEY_PARAMS, &self.ui.emu.params);
        eframe::set_value(storage, KEY_LAYOUT, &self.layout);
        eframe::set_value(storage, KEY_SLICE, &self.ui.slicer.settings);
        eframe::set_value(storage, KEY_SLICE_AUTO, &self.ui.slicer.auto);
    }
}
