# PC ambient lighting

## Goal

Make the LEDs around the display react to the colors in Windows or Linux games
and videos. A desktop companion captures and samples the display, blends the
samples into colors for the LED layout, and sends those colors to the ESP32-S3
over Wi-Fi. The ESP32 continues to generate the WS2812 signal and drive the
LEDs.

The ESP32 should not receive screenshots or process image data. Keeping capture
and color analysis on the PC minimizes ESP32 RAM use, network traffic, and
firmware work.

## Components

- **Desktop companion:** Provides setup and controls, captures the display,
  computes ambient colors, and streams updates over WebSocket. It should support
  Windows and Linux; capture APIs and permissions may differ by operating
  system.
- **ESP32 firmware:** Connects to Wi-Fi, accepts WebSocket commands, stores only
  the current lighting state, and refreshes the LEDs. It does not need to host
  the control UI or embedded web assets.
- **LED strip:** Uses the existing fixed layout of four sides with 120 LEDs per
  side (480 total).

## Data flow

1. The user selects the display and enables ambient mode in the desktop app.
2. The app samples pixels from regions along the display edges and computes a
   representative RGB color for each corresponding LED side. Sampling should
   downscale and blend regions on the PC; raw frames must not be sent to the
   ESP32.
3. The app sends a compact binary WebSocket message with four RGB values and
   optional brightness/settings.
4. The ESP32 applies each side's color to its 120 LEDs and emits the normal
   WS2812 frame. It can smooth between incoming updates to reduce abrupt
   changes.
5. If updates stop or the connection closes, firmware behavior should be
   defined (for example, fade to off or hold the last colors). The desktop app
   indicates connection and capture status.

## Proposed WebSocket protocol

The current firmware has `/ws` and accepts a 486-byte control packet (power,
RGB, brightness, effect, and a 480-LED mask), plus a custom animation packet.
Ambient mode needs a distinct packet so the smaller message is unambiguous.

Proposed v1 binary packet, 14 bytes:

| Offset | Size | Meaning |
| --- | ---: | --- |
| 0 | 1 | Message type: `0xB1` (ambient colors) |
| 1 | 1 | Flags; bit 0 is power enabled, remaining bits reserved and zero |
| 2 | 1 | Brightness, 0–255 |
| 3 | 12 | Four RGB triplets in physical strip order: top, right, bottom, left |

The PC sends only the latest ambient state at a modest rate (initial target:
30 updates/second or less). If the link is congested, it should discard stale
samples instead of queueing old colors. At 30 updates/second, the payload is
420 bytes/second before WebSocket/TCP overhead.

This packet is a proposal, not implemented behavior. The firmware and desktop
app must adopt the same version and byte order. The existing 486-byte manual
control message can remain available for individual LED selection and setup;
ambient mode should take precedence while active.

## Desktop framework direction

Tauri is a reasonable first choice because the existing React interface can be
reused, while native Rust code can handle capture and processing. Screen capture
support must be validated separately on Windows and the target Linux desktop
environment. Flutter is also viable but would require rebuilding the current
React UI. Framework choice does not change the WebSocket protocol.

## Performance and memory constraints

- Never transfer full-resolution screenshots to the microcontroller.
- Keep only the latest four RGB colors and small protocol state on the ESP32.
- Avoid allocating per incoming packet in the firmware receive path.
- Continue generating per-LED WS2812 timing data locally on the ESP32.
- Tune capture resolution, sampling rate, blending, and smoothing on the PC to
  balance responsiveness, CPU use, and visual stability.

## Implementation outline

1. Add the ambient packet handler and four side-color state to firmware.
2. Render the four side colors across the existing LED ranges, retaining the
   existing manual controls as a separate mode.
3. Create the Windows/Linux desktop companion with device connection, display
   selection, ambient enable/disable, and capture status.
4. Implement OS-specific screen capture and PC-side edge sampling/blending.
5. Add reconnect handling and a defined stale-connection behavior.
6. Remove embedded UI asset serving once the companion can perform all required
   setup and control tasks; keep the ESP32 WebSocket endpoint.

## Acceptance criteria

- A Windows or Linux desktop session can select a display and enable ambient
  lighting.
- The four LED sides respond to corresponding screen-edge colors and transition
  smoothly during typical gameplay/video playback.
- No screenshot or full image is sent to the ESP32; ambient updates use the
  compact packet above.
- Loss of the desktop connection produces the documented safe lighting state
  and a visible disconnected status in the app.
- Manual LED controls remain usable when ambient mode is disabled.
