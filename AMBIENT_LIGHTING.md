# PC ambient lighting

## Goal

Make the LEDs around the display react to colors on a desktop PC. A Tauri
companion captures and samples the selected display, blends the samples into
colors for the LED layout, and sends those colors to the ESP32-S3 over Wi-Fi.
The app is developed and first exercised on Linux, with Windows as the intended
use target. Ambient capture is disabled in browsers and on mobile devices.

The ESP32 should not receive screenshots or process image data. Keeping capture
and color analysis on the PC minimizes ESP32 RAM use, network traffic, and
firmware work.

## Components

- **Desktop companion:** A Tauri app uses native Rust to list monitors,
  capture a selected display, compute ambient colors, and stream updates over
  WebSocket. Linux X11 is the initial development target; Windows is supported
  for use on desktop PCs.
- **ESP32 firmware:** Connects to Wi-Fi, accepts WebSocket commands, stores only
  the current lighting state, and refreshes the LEDs. It does not need to host
  the control UI or embedded web assets.
- **LED strip:** Uses the existing fixed layout of four sides with 120 LEDs per
  side (480 total).

## Data flow

1. The user selects the display and enables ambient mode in the desktop app.
2. The app samples pixels from a band just inside each display edge (from 4%
   to 20% of the shorter screen dimension, skipping the outermost pixels such
   as taskbars and letterbox bars) and computes 16 RGB zone colors per side,
   lightly blurred into their neighbours. Sampling should
   downscale and blend regions on the PC; raw frames must not be sent to the
   ESP32.
3. The app sends a compact binary WebSocket message with the 64 zone colors and
   brightness.
4. The ESP32 places each zone color at the zone's center and interpolates
   between neighbouring zones (in linear light, wrapping around the corners) to
   produce a smooth gradient across all 480 LEDs, then emits the WS2812 frame. It can smooth between incoming updates to reduce abrupt
   changes.
5. If updates stop or the connection closes, firmware turns the LEDs off. The
   desktop app indicates connection and capture status.

## WebSocket protocol

The current firmware has `/ws` and accepts a 66-byte control packet (power,
RGB, brightness, effect, and a 480-LED mask), plus a custom animation packet.
Ambient mode uses a distinct packet so the smaller message is unambiguous.

The binary packet is 195 bytes:

| Offset | Size | Meaning |
| --- | ---: | --- |
| 0 | 1 | Message type: `0xB1` (ambient colors) |
| 1 | 1 | Flags; bit 0 is power enabled, remaining bits reserved and zero |
| 2 | 1 | Brightness, 0–255 |
| 3 | 192 | 64 RGB triplets: 16 zones per side, clockwise in physical strip order (top left→right, right top→bottom, bottom right→left, left bottom→top) |

The PC sends only the latest ambient state at a modest rate (initial target:
30 updates/second or less). If the link is congested, it should discard stale
samples instead of queueing old colors. At 30 updates/second, the payload is
about 5.9 KB/second before WebSocket/TCP overhead. Colors are sRGB as sampled from
the screen; firmware converts them to linear light before driving the LEDs.

This packet is implemented by the current firmware and desktop UI. The existing
66-byte manual control message remains available for individual LED selection
and setup; a manual update exits ambient mode. When an ambient WebSocket closes,
firmware turns the strip off. The desktop UI sends an ambient packet with power
disabled when capture is stopped.

## Desktop framework direction

The React UI runs inside a Tauri desktop app. Native Rust code enumerates
monitors, captures the selected display, downsamples samples along each edge,
and blends successive colors before emitting only the 64 zone colors to the UI.
The UI sends the compact ambient packet directly to the configured ESP32
WebSocket endpoint. Ambient capture controls are enabled in Linux and Windows
Tauri desktop builds, and disabled in browsers and mobile apps. Linux X11 is
the first development target; Wayland capture needs validation against the
compositor and capture backend.

## Performance and memory constraints

- Never transfer full-resolution screenshots to the microcontroller.
- Keep only the latest 64 zone colors and small protocol state on the ESP32.
- Avoid allocating per incoming packet in the firmware receive path.
- Continue generating per-LED WS2812 timing data locally on the ESP32.
- Tune capture resolution, sampling rate, blending, and smoothing on the PC to
  balance responsiveness, CPU use, and visual stability.

## Implementation outline

1. Implement the ambient packet, side-color rendering, Tauri capture for Linux
   and Windows, edge sampling/blending, and stale-connection off behavior.
   (Initial increment.)
2. Validate capture on this Linux X11 development desktop and a Windows desktop;
   then assess Linux Wayland support.
3. Add reconnect handling that resumes ambient updates after a device link
   interruption.
4. Remove embedded UI asset serving when the desktop companion can perform all
   required setup and control tasks.

## Acceptance criteria

- A Linux or Windows desktop session can select a display and enable ambient
  lighting; browsers and mobile devices disable ambient controls.
- Each LED side shows a gradient of the corresponding screen-edge colors and transition
  smoothly during typical gameplay/video playback.
- No screenshot or full image is sent to the ESP32; ambient updates use the
  compact packet above.
- Loss of the desktop connection produces the documented safe lighting state
  and a visible disconnected status in the app.
- Manual LED controls remain usable when ambient mode is disabled.
