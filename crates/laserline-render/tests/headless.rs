//! Renders into offscreen textures and checks pixels. Runs on every native backend
//! found; the GL backend exercises the same GLSL translation WebGL2 uses. Tests pass
//! without checking anything when no adapter is available.

use laserline_core::emulator::Emulator;
use laserline_core::gpu_types::Overlay;
use laserline_core::params::{DeformKind, ProfileKind};
use laserline_core::slicer::Slicer;
use laserline_core::{mesh, scan};
use laserline_render::{LaserRenderer, MeshViewInput, MeshViewRenderer};
use std::sync::atomic::AtomicBool;

const W: u32 = 256;
const H: u32 = 128;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

struct Gpu {
    name: String,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn gpus() -> Vec<Gpu> {
    let mut out = Vec::new();
    for backends in [wgpu::Backends::VULKAN, wgpu::Backends::GL, wgpu::Backends::METAL, wgpu::Backends::DX12] {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor { backends, ..Default::default() });
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) else {
            continue;
        };
        let limits = wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
        let desc = wgpu::DeviceDescriptor { required_limits: limits, ..Default::default() };
        let Ok((device, queue)) = pollster::block_on(adapter.request_device(&desc)) else {
            continue;
        };
        let name = format!("{:?} {}", adapter.get_info().backend, adapter.get_info().name);
        eprintln!("testing on {name}");
        out.push(Gpu { name, adapter, device, queue });
    }
    if out.is_empty() {
        eprintln!("no GPU adapter: skipping");
    }
    out
}

/// Runs `draw` in a pass clearing an offscreen W×H target and returns its RGBA bytes.
fn render(gpu: &Gpu, prepare: impl FnOnce(&mut wgpu::CommandEncoder), draw: impl FnOnce(&mut wgpu::RenderPass<'_>)) -> Vec<u8> {
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (W * H * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    prepare(&mut encoder);
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        draw(&mut pass);
    }
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(W * 4), rows_per_image: None },
        },
        target.size(),
    );
    gpu.queue.submit([encoder.finish()]);
    readback.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    readback.slice(..).get_mapped_range().to_vec()
}

fn red(px: &[u8], x: u32, y: u32) -> u8 {
    px[((y * W + x) * 4) as usize]
}

/// Row of the brightest red pixel in column `x`.
fn peak_row(px: &[u8], x: u32) -> u32 {
    (0..H).max_by_key(|&y| red(px, x, y)).unwrap()
}

fn line(gpu: &Gpu, emu: &Emulator) -> Vec<u8> {
    let r = LaserRenderer::new(&gpu.device, FORMAT);
    let frame = emu.frame([0.0, 0.0], [W as f32, H as f32], false);
    r.prepare(&gpu.queue, &frame.uniforms, frame.deform.as_deref());
    render(gpu, |_| {}, |pass| r.paint(pass))
}

fn hard_red_line() -> Emulator {
    let mut emu = Emulator::default();
    let p = &mut emu.params;
    p.line.width_px = 3.0;
    p.profile.kind = ProfileKind::Hard;
    p.profile.end_softness_px = 0.0;
    p.color.mode = laserline_core::params::ColorMode::Custom;
    p.color.custom_rgb = [1.0, 0.0, 0.0];
    emu
}

#[test]
fn straight_line() {
    for gpu in gpus() {
        let px = line(&gpu, &hard_red_line());
        // Horizontal line through the middle: rows 63..=65 lit, nothing far away.
        assert_eq!(red(&px, W / 2, H / 2), 255, "{}", gpu.name);
        assert_eq!(red(&px, W / 2, H / 2 - 1), 255, "{}", gpu.name);
        assert_eq!(red(&px, W / 2, 10), 0, "{}", gpu.name);
        assert_eq!(px[((H / 2 * W + W / 2) * 4 + 1) as usize], 0, "{}: green", gpu.name);
    }
}

#[test]
fn deformation_texture_shifts_the_line() {
    for gpu in gpus() {
        let mut emu = hard_red_line();
        let d = &mut emu.params.deform;
        d.kind = DeformKind::Box;
        d.position_px = 0.0;
        d.width_px = 80.0;
        d.slope_px = 0.0;
        d.height_px = 20.0;
        d.scroll_px_s = 0.0;
        let px = line(&gpu, &emu);
        // The 80 % long line spans x ≈ 26..230; the box covers x ≈ 88..168.
        let flat = peak_row(&px, 48);
        let raised = peak_row(&px, W / 2);
        assert!((flat as i32 - (H / 2) as i32).abs() <= 1, "{}: flat part at {flat}", gpu.name);
        assert!((raised as i32 - flat as i32 - 20).abs() <= 1, "{}: box displaced by {}", gpu.name, raised as i32 - flat as i32);
    }
}

#[test]
fn scan_drives_the_line() {
    // A 20 mm high box in the middle of a 100 mm wide scan, shown at 1 px/mm.
    let m = mesh::box_mesh([0.0; 3], [40.0, 10.0, 20.0]);
    let s = scan::SliceSettings { lateral_res_mm: 0.5, travel_step_mm: 1.0, margin_lateral_mm: 30.0, occlusion: false, ..Default::default() };
    let scan = scan::slice(&m, &s, "", &AtomicBool::new(false), &scan::Progress::new(0)).unwrap();
    for gpu in gpus() {
        let mut emu = hard_red_line();
        emu.params.scan.enabled = true;
        emu.params.scan.scale = laserline_core::params::ScanScale::Fixed;
        emu.params.scan.px_per_mm = 1.0;
        emu.set_scan(scan.clone());
        emu.seek_row(15); // v = 5 mm, through the box after the 10 mm lead-in
        let px = line(&gpu, &emu);
        let floor = peak_row(&px, W / 2 - 45);
        let top = peak_row(&px, W / 2);
        assert!((floor as i32 - top as i32 - 20).abs() <= 1, "{}: higher surface moves the line up by {}", gpu.name, floor as i32 - top as i32);
    }
}

#[test]
fn mesh_view_draws_the_model() {
    for gpu in gpus() {
        let mut sl = Slicer::new(Default::default(), false);
        sl.set_model(mesh::box_mesh([0.0; 3], [20.0, 20.0, 20.0]), "box", "box");
        sl.update_preview(None);
        let view = sl.view_3d(None, 0, false).unwrap();
        let samples = MeshViewRenderer::supported_samples(&gpu.adapter);
        let mut r = MeshViewRenderer::new(&gpu.device, FORMAT, samples);
        let input = MeshViewInput {
            mesh: view.mesh.as_deref(),
            overlay: &Overlay::default(),
            uniforms: view.camera.uniforms(W as f32 / H as f32, view.z_max),
            size_px: [W, H],
            srgb_target: false,
        };
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        r.prepare(&gpu.device, &gpu.queue, &mut encoder, &input);
        gpu.queue.submit([encoder.finish()]);
        let px = render(&gpu, |_| {}, |pass| r.paint(pass));
        // The box is centered and lit grey; the corner shows the background.
        let center = &px[((H / 2 * W + W / 2) * 4) as usize..][..3];
        assert!(center.iter().all(|&c| c > 60), "{} ({samples}x): center {center:?}", gpu.name);
        assert!(red(&px, 1, 1) < 30, "{}: background", gpu.name);
    }
}

#[test]
fn cpu_matches_gpu_across_presets_and_scan() {
    use laserline_core::cpu;
    use laserline_core::presets;

    let m = mesh::box_mesh([0.0; 3], [40.0, 10.0, 20.0]);
    let s = scan::SliceSettings { lateral_res_mm: 0.5, travel_step_mm: 1.0, margin_lateral_mm: 30.0, occlusion: true, ..Default::default() };
    let scan = scan::slice(&m, &s, "", &AtomicBool::new(false), &scan::Progress::new(0)).unwrap();

    let mut cases: Vec<(String, Emulator)> = presets::builtin()
        .into_iter()
        .map(|(name, p)| {
            let mut e = Emulator::new(p);
            e.tick(0.15);
            (name.to_string(), e)
        })
        .collect();

    // Add a top-hat profile and a scan-driven profile with speckle + noise.
    let mut tophat = Emulator::default();
    tophat.params.profile.kind = ProfileKind::TopHat;
    tophat.params.profile.edge_softness_px = 3.0;
    tophat.params.line.width_px = 10.0;
    tophat.params.line.tilt_deg = 12.0;
    cases.push(("Top-hat tilted".into(), tophat));

    let mut scanned = Emulator::default();
    scanned.params.scan.enabled = true;
    scanned.params.scan.scale = laserline_core::params::ScanScale::Fixed;
    scanned.params.scan.px_per_mm = 1.5;
    scanned.params.noise.ambient_pct = 5.0;
    scanned.params.noise.speckle_pct = 40.0;
    scanned.params.noise.noise_pct = 2.0;
    scanned.set_scan(scan);
    scanned.seek_row(12);
    cases.push(("Scan + speckle + noise".into(), scanned));

    for gpu in gpus() {
        let mut cpu_gray = vec![0u8; (W * H) as usize];
        for (name, emu) in &cases {
            let gpu_rgba = line(&gpu, emu);
            let frame = emu.frame([0.0, 0.0], [W as f32, H as f32], false);
            cpu::render_gray8(&frame, W, H, &mut cpu_gray);

            // Compare the brightest RGB channel of the GPU output with `render_gray8`,
            // and also compare all 3 RGB channels against `eval_pixel_rgb`.
            let mut abs_diff_gray = 0u64;
            let mut max_diff_gray = 0u8;
            let mut abs_diff_rgb = 0u64;
            for y in 0..H {
                for x in 0..W {
                    let idx = (y * W + x) as usize;
                    let g_rgb = [gpu_rgba[idx * 4], gpu_rgba[idx * 4 + 1], gpu_rgba[idx * 4 + 2]];
                    let g_max = g_rgb[0].max(g_rgb[1]).max(g_rgb[2]);
                    let c_gray = cpu_gray[idx];
                    let dg = g_max.abs_diff(c_gray);
                    abs_diff_gray += dg as u64;
                    max_diff_gray = max_diff_gray.max(dg);

                    let c_rgb = cpu::eval_pixel_rgb(&frame.uniforms, frame.deform.as_deref(), x as f32 + 0.5, y as f32 + 0.5)
                        .map(|v| (v * 255.0).round() as u8);
                    for ch in 0..3 {
                        abs_diff_rgb += g_rgb[ch].abs_diff(c_rgb[ch]) as u64;
                    }
                }
            }
            let n = (W * H) as f64;
            let mae_gray = abs_diff_gray as f64 / n;
            let mae_rgb = abs_diff_rgb as f64 / (n * 3.0);
            eprintln!("{} | {}: gray MAE = {mae_gray:.4} (max {max_diff_gray}), rgb MAE = {mae_rgb:.4}", gpu.name, name);
            assert!(
                mae_gray <= 0.5,
                "{} ({name}): gray mean absolute error {mae_gray:.4} exceeds 0.5 / 255 (max diff {max_diff_gray})",
                gpu.name
            );
            assert!(
                mae_rgb <= 0.5,
                "{} ({name}): rgb mean absolute error {mae_rgb:.4} exceeds 0.5 / 255",
                gpu.name
            );
        }
    }
}
