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

    let mut previous = [[0u8; 3]; 4];
    while !stopped.load(Ordering::Relaxed) {
        let started = Instant::now();
        let capture = (|| -> Result<[[u8; 3]; 4], String> {
            let image = monitor.capture_image().map_err(|error| error.to_string())?;
            let width = image.width();
            let height = image.height();
            if width == 0 || height == 0 {
                return Err("Selected display returned an empty frame".into());
            }
            let step_x = (width / 160).max(1) as usize;
            let step_y = (height / 90).max(1) as usize;
            let band_x = (width / 12).max(1);
            let band_y = (height / 12).max(1);
            let mut sums = [[0u64; 3]; 4];
            let mut counts = [0u64; 4];
            for y in (0..height).step_by(step_y) {
                for x in (0..width).step_by(step_x) {
                    let sides = [y < band_y, x >= width - band_x, y >= height - band_y, x < band_x];
                    let pixel = image.get_pixel(x, y).0;
                    for side in 0..4 {
                        if sides[side] {
                            for channel in 0..3 {
                                sums[side][channel] += pixel[channel] as u64;
                            }
                            counts[side] += 1;
                        }
                    }
                }
            }
            let mut colors = [[0u8; 3]; 4];
            for side in 0..4 {
                for channel in 0..3 {
                    let average = sums[side][channel] / counts[side].max(1);
                    colors[side][channel] = average as u8;
                }
            }
            Ok(colors)
        })();

        match capture {
            Ok(sampled) => {
                for side in 0..4 {
                    for channel in 0..3 {
                        previous[side][channel] =
                            ((previous[side][channel] as u16 * 55 + sampled[side][channel] as u16 * 45) / 100) as u8;
                    }
                }
                if app.emit("ambient-colors", previous).is_err() {
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

pub fn run() {
    tauri::Builder::default()
        .manage(CaptureState::default())
        .invoke_handler(tauri::generate_handler![list_monitors, start_capture, stop_capture])
        .run(tauri::generate_context!())
        .expect("error while running practice-esp desktop companion");
}
