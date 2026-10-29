#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

pub mod calibration;
pub mod camera;
pub mod emulator;
pub mod formats;
pub mod gpu_types;
pub mod mesh;
pub mod metric;
pub mod model;
pub mod params;
pub mod presets;
pub mod scan;
pub mod scan_file;
pub mod slicer;
pub mod viz;

pub use gpu_types::DEFORM_SAMPLES;
/// Re-exported so users of the camera and overlay types get the matching version.
pub use glam;
