use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, State};

#[derive(Default)]
struct CaptureState(Mutex<Option<Arc<AtomicBool>>>);

#[derive(Serialize)]
struct MonitorInfo {
    id: u32,
    name: String,
    width: u32,
    height: u32,
    primary: bool,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn monitor_info(monitor: &xcap::Monitor) -> Result<MonitorInfo, String> {
    let id = monitor.id().map_err(|error| error.to_string())?;
    Ok(MonitorInfo {
        id,
        name: monitor.friendly_name().unwrap_or_else(|_| format!("Display {id}")),
        width: monitor.width().map_err(|error| error.to_string())?,
        height: monitor.height().map_err(|error| error.to_string())?,
        primary: monitor.is_primary().unwrap_or(false),
    })
}

#[tauri::command]
fn list_monitors() -> Result<Vec<MonitorInfo>, String> {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        return xcap::Monitor::all()
            .map_err(|error| error.to_string())?
            .iter()
            .map(monitor_info)
            .collect();
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    Err("Ambient capture is supported on Linux and Windows desktop only".into())
}

#[tauri::command]
fn start_capture(
    app: tauri::AppHandle,
    state: State<'_, CaptureState>,
    monitor_id: u32,
) -> Result<(), String> {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        stop_capture_inner(&state)?;
        let stopped = Arc::new(AtomicBool::new(false));
        *state.0.lock().map_err(|_| "Capture state is unavailable")? = Some(stopped.clone());
        std::thread::Builder::new()
            .name("ambient-screen-capture".into())
            .spawn(move || capture_loop(app, monitor_id, stopped))
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (app, state, monitor_id);
        Err("Ambient capture is supported on Linux and Windows desktop only".into())
    }
}

#[tauri::command]
fn stop_capture(state: State<'_, CaptureState>) -> Result<(), String> {
    stop_capture_inner(&state)
}

fn stop_capture_inner(state: &CaptureState) -> Result<(), String> {
    if let Some(stopped) = state.0.lock().map_err(|_| "Capture state is unavailable")?.take() {
        stopped.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// How strongly saturated edge pixels outweigh grey ones when averaging.
const SATURATION_WEIGHT: f32 = 8.0;
/// Per-channel change (0-255) treated as a scene change rather than noise.
const SNAP_DELTA: f32 = 48.0;
/// Smoothing time constants in seconds for small and large color changes.
const SLOW_TIME_CONSTANT: f32 = 0.12;
const FAST_TIME_CONSTANT: f32 = 0.03;

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn capture_loop(app: tauri::AppHandle, monitor_id: u32, stopped: Arc<AtomicBool>) {
    use std::time::{Duration, Instant};
    use xcap::Monitor;

    let monitor = match Monitor::all()
        .map_err(|error| error.to_string())
        .and_then(|monitors| {
            monitors
                .into_iter()
                .find(|monitor| monitor.id().ok() == Some(monitor_id))
                .ok_or_else(|| "Selected display is no longer available".to_string())
        })
    {
        Ok(monitor) => monitor,
        Err(error) => {
            let _ = app.emit("ambient-capture-error", error);
            return;
        }
    };

    let mut smoother = Smoother::default();
    while !stopped.load(Ordering::Relaxed) {
        let started = Instant::now();
        let capture = monitor
            .capture_image()
            .map_err(|error| error.to_string())
            .and_then(|image| sample_edges(&image));
        match capture {
            Ok(sampled) => {
                if app.emit("ambient-colors", smoother.update(sampled, Instant::now())).is_err() {
                    break;
                }
            }
            Err(error) => {
                let _ = app.emit("ambient-capture-error", error);
                break;
            }
        }
        let remaining = Duration::from_millis(33).saturating_sub(started.elapsed());
        std::thread::sleep(remaining);
    }
}

/// Average the pixels in a band along each display edge: top, right, bottom, left.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn sample_edges(image: &xcap::image::RgbaImage) -> Result<[[f32; 3]; 4], String> {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return Err("Selected display returned an empty frame".into());
    }
    let step_x = (width / 160).max(1) as usize;
    let step_y = (height / 90).max(1) as usize;
    let band_x = (width / 12).max(1);
    let band_y = (height / 12).max(1);
    let mut sums = [[0f32; 3]; 4];
    let mut weights = [0f32; 4];
    for y in (0..height).step_by(step_y) {
        for x in (0..width).step_by(step_x) {
            let sides = [y < band_y, x >= width - band_x, y >= height - band_y, x < band_x];
            if !sides.contains(&true) {
                continue;
            }
            let pixel = image.get_pixel(x, y).0;
            // Weight vivid pixels above grey ones. A plain average of a busy
            // edge tends toward grey, which looks washed out on the LEDs; this
            // keeps the dominant hue.
            let max = pixel[0].max(pixel[1]).max(pixel[2]) as f32;
            let min = pixel[0].min(pixel[1]).min(pixel[2]) as f32;
            let chroma = (max - min) / 255.0;
            let weight = 1.0 + SATURATION_WEIGHT * chroma * chroma;
            for side in 0..4 {
                if sides[side] {
                    for channel in 0..3 {
                        sums[side][channel] += pixel[channel] as f32 * weight;
                    }
                    weights[side] += weight;
                }
            }
        }
    }
    let mut colors = [[0f32; 3]; 4];
    for side in 0..4 {
        for channel in 0..3 {
            colors[side][channel] = sums[side][channel] / weights[side].max(1.0);
        }
    }
    Ok(colors)
}

/// Temporal filter for the sampled edge colors. State is kept as floats so it
/// reaches its target instead of stalling a few steps short through integer
/// truncation.
#[derive(Default)]
struct Smoother {
    colors: [[f32; 3]; 4],
    previous_at: Option<std::time::Instant>,
}

impl Smoother {
    fn update(&mut self, sampled: [[f32; 3]; 4], now: std::time::Instant) -> [[u8; 3]; 4] {
        let elapsed = self.previous_at.map_or(1.0, |at| (now - at).as_secs_f32());
        self.previous_at = Some(now);
        for (smoothed, sampled) in self.colors.iter_mut().zip(sampled) {
            // Smooth small changes to avoid flicker, but follow large ones
            // (scene cuts, explosions) almost immediately.
            let delta = (0..3)
                .map(|channel| (sampled[channel] - smoothed[channel]).abs())
                .fold(0.0, f32::max);
            let time_constant = if delta >= SNAP_DELTA { FAST_TIME_CONSTANT } else { SLOW_TIME_CONSTANT };
            let alpha = 1.0 - (-elapsed / time_constant).exp();
            for channel in 0..3 {
                smoothed[channel] += (sampled[channel] - smoothed[channel]) * alpha;
            }
        }
        self.colors.map(|side| side.map(|channel| channel.round().clamp(0.0, 255.0) as u8))
    }
}

pub fn run() {
    tauri::Builder::default()
        .manage(CaptureState::default())
        .invoke_handler(tauri::generate_handler![list_monitors, start_capture, stop_capture])
        .run(tauri::generate_context!())
        .expect("error while running practice-esp desktop companion");
}

#[cfg(all(test, any(target_os = "linux", target_os = "windows")))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use xcap::image::{Rgba, RgbaImage};

    fn frame(fill: [u8; 3]) -> RgbaImage {
        RgbaImage::from_pixel(1920, 1080, Rgba([fill[0], fill[1], fill[2], 255]))
    }

    #[test]
    fn solid_edges_keep_exact_color() {
        let mut image = frame([0, 0, 0]);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = match () {
                _ if y < 90 => Rgba([255, 0, 0, 255]),
                _ if y >= 990 => Rgba([0, 0, 255, 255]),
                _ if x >= 1760 => Rgba([0, 255, 0, 255]),
                _ if x < 160 => Rgba([255, 128, 0, 255]),
                _ => Rgba([255, 255, 255, 255]),
            };
        }
        let colors = sample_edges(&image).unwrap();
        // Top and bottom bands span the full width, so corners mix in the
        // side colors; the dominant color must still win clearly.
        assert!(colors[0][0] > 200.0 && colors[0][1] < 60.0, "top {:?}", colors[0]);
        assert!(colors[2][2] > 200.0 && colors[2][0] < 60.0, "bottom {:?}", colors[2]);
        assert!(colors[1][1] > 150.0, "right {:?}", colors[1]);
        assert!(colors[3][0] > 150.0, "left {:?}", colors[3]);
    }

    #[test]
    fn vivid_pixels_outweigh_grey() {
        // 40% red, 60% grey stripes across the whole frame.
        let mut image = frame([128, 128, 128]);
        for (x, _, pixel) in image.enumerate_pixels_mut() {
            if x % 10 < 4 {
                *pixel = Rgba([220, 30, 30, 255]);
            }
        }
        let top = sample_edges(&image).unwrap()[0];
        let plain = [165.0, 89.0, 89.0];
        assert!(top[0] - top[1] > (plain[0] - plain[1]) * 1.5, "not saturated enough: {top:?}");
    }

    #[test]
    fn scene_cut_settles_within_100ms_and_reaches_target() {
        let mut smoother = Smoother::default();
        let start = Instant::now();
        smoother.update([[0.0; 3]; 4], start);
        let mut reached_90 = None;
        let mut last = [[0; 3]; 4];
        for step in 1..=60u32 {
            let now = start + Duration::from_millis(33 * step as u64);
            last = smoother.update([[255.0; 3]; 4], now);
            if reached_90.is_none() && last[0][0] >= 230 {
                reached_90 = Some(33 * step);
            }
        }
        assert!(reached_90.unwrap() <= 100, "90% after {reached_90:?} ms");
        assert_eq!(last[0], [255, 255, 255]);
    }

    #[test]
    fn small_changes_are_smoothed() {
        let mut smoother = Smoother::default();
        let start = Instant::now();
        smoother.update([[100.0; 3]; 4], start);
        let next = smoother.update([[120.0; 3]; 4], start + Duration::from_millis(33));
        assert!(next[0][0] > 100 && next[0][0] < 115, "{:?}", next[0]);
    }

    /// Captures the real primary display: `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_capture_rate() {
        let monitor = xcap::Monitor::all()
            .unwrap()
            .into_iter()
            .find(|monitor| monitor.is_primary().unwrap_or(false))
            .expect("no primary display");
        let mut capture_ms = Vec::new();
        let mut sample_ms = Vec::new();
        let mut colors = [[0f32; 3]; 4];
        for _ in 0..30 {
            let started = Instant::now();
            let image = monitor.capture_image().unwrap();
            let captured = Instant::now();
            colors = sample_edges(&image).unwrap();
            capture_ms.push((captured - started).as_secs_f64() * 1000.0);
            sample_ms.push(captured.elapsed().as_secs_f64() * 1000.0);
        }
        let average = |values: &[f64]| values.iter().sum::<f64>() / values.len() as f64;
        let worst = |values: &[f64]| values.iter().cloned().fold(0.0, f64::max);
        println!(
            "capture avg {:.1} ms (max {:.1}), sample avg {:.2} ms (max {:.2})",
            average(&capture_ms), worst(&capture_ms), average(&sample_ms), worst(&sample_ms)
        );
        println!("edge colors top/right/bottom/left: {colors:?}");
        assert!(average(&capture_ms) + average(&sample_ms) < 33.0, "cannot sustain 30 Hz");
    }
}
