# ESP32-S3 Rust + Vite React Server

This firmware connects the ESP32-S3 WROOM-1 to an existing Wi-Fi access point,
gets an IPv4 address through DHCP, and serves the Vite React app from flash.
`build.rs` compresses every file in `web/dist` with Brotli quality 11 and embeds
the `.br` bytes into the firmware image.

## Build the web app

```sh
cd web
npm install
npm run build
cd ..
```

## Run the browser UI for manual controls

Connect the computer and ESP32 to the same Wi-Fi network, then run Vite with
the ESP32's IP address. Vite proxies the page's `/ws` connection to the device:

```sh
cd web
ESP32_HOST=http://192.168.1.42 yarn dev
```

Replace `192.168.1.42` with the address printed by the ESP32 serial monitor.
The default proxy target is `http://192.168.4.1`.

Ambient capture is available only in the Tauri desktop companion. Browser tabs
and mobile devices show the control disabled. Screen capture and edge sampling
run in native Rust; the React UI receives only the four averaged RGB values.

Install the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)
for your development OS, then run the desktop app on Linux or Windows:

On Ubuntu/Debian, install Tauri and XCap's native build dependencies first:

```sh
sudo apt update
sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev libclang-dev \
  libxcb1-dev libxrandr-dev libdbus-1-dev libpipewire-0.3-dev libwayland-dev \
  libegl-dev libgbm-dev
```

The Tauri launcher selects the machine's host Rust target automatically, keeping
the ESP32 cross-compilation target from affecting the desktop build.

```sh
cd web
yarn install
yarn tauri dev
```

Enter the ESP32 IP address printed by its serial monitor and select **Connect**.
Choose a display, then start ambient capture. Linux X11 is the first capture
target; Wayland behavior depends on the desktop capture backend and still needs
validation. Run `yarn tauri build` on Linux for `.deb`/AppImage bundles and on
Windows for `.msi`/NSIS installers.

The PC averages four display-edge bands and sends at most 30 compact 14-byte
ambient updates per second. No screenshot is sent to the ESP32. Stopping capture
sends power off; closing the ambient WebSocket also turns the strip off. Manual
LED updates exit ambient mode.

## Build and flash firmware

The Wi-Fi access point name and password are set in `src/bin/main.rs`:

```rust
const SSID: &str = "Enchanter";
const PASSWORD: &str = "Khanhome";
```

For the first flash, connect USB and install the OTA-capable firmware and new
partition table:

```sh
cargo run --release
```

After that, update over Wi-Fi from a computer on the same network. Use the IP
printed by the serial monitor:

```sh
./ota-update.sh 192.168.1.42
```

This builds the firmware, saves an ESP32-S3 application image, uploads it to
`http://<device-ip>/update`, and the ESP32 reboots into the new image. OTA
images must fit within the 1.875 MB app slots. Keep USB available for the first
OTA-capable flash or recovery if an update is interrupted by power loss.

At boot the firmware points `otadata` at the slot it is running from and marks
it valid, so each OTA writes the other slot. USB flashes (`cargo run`) always
write `ota_0` and erase `otadata`, so the device boots the image just flashed.

The serial monitor prints the assigned URL, for example:

```text
Serving React app at http://192.168.1.42/
```

The firmware saves the LED power, color, effect, and per-LED mask to the
`led_state` flash partition after changes settle, then restores them at boot.
`espflash.toml` selects the custom 4 MB partition table in `partitions.csv`.
OTA cannot change the partition table, so edits to it need a USB flash.

Browsers request Brotli by default, and the ESP replies with
`Content-Encoding: br`.

## Simulate with Wokwi before flashing

[Wokwi](https://wokwi.com) simulates the ESP32-S3 with Wi-Fi, the 480-pixel
WS2812 frame on GPIO 4, the GPIO 5 power output (green LED), and the custom
partition table, so OTA slots and the `led_state` flash partition behave as on
hardware.

The `wokwi` Cargo feature connects to the simulator's open `Wokwi-GUEST`
access point instead of the real network. Build the firmware and merged flash
image:

```sh
./wokwi-build.sh
```

`diagram.json` describes the circuit. The four strips follow the firmware's
order: top (left→right), right (top→bottom), bottom (right→left), and left
(bottom→top). `wokwi.toml` points at `target/wokwi-flash.bin` and the ELF.

### Run in VS Code or RustRover

Install the Wokwi extension (VS Code: `wokwi.wokwi-vscode`, or the Wokwi
Simulator plugin for JetBrains), run **Wokwi: Request a New License**, then
open `diagram.json` and start the simulator. With the private gateway, the web
UI is reachable at <http://localhost:8180>. Point the dev server at it with:

```sh
cd web
ESP32_HOST=http://localhost:8180 yarn dev
```

### Run headless with wokwi-cli

Create a token at <https://wokwi.com/dashboard/ci> and export it as
`WOKWI_CLI_TOKEN`, then:

```sh
wokwi-cli --diagram-file ../diagram.json --timeout 30000 \
    --expect-text "Serving React app" wokwi-ci
```

`wokwi-ci/wokwi.toml` leaves out the GDB server. The root `wokwi.toml` enables
it, which makes `wokwi-cli` pause at reset until a debugger attaches.

### Debug with GDB

`wokwi.toml` exposes a GDB server on port 3333. Start the simulation in your
editor (use **Wokwi: Start Simulator and Wait for Debugger** to pause at
reset), then attach:

```sh
xtensa-esp32s3-elf-gdb target/xtensa-esp32s3-none-elf/release/practice-esp \
    -ex "target remote localhost:3333"
```

Use `break`, not `hbreak`; the simulator only supports software breakpoints.
For `wokwi-cli`, pass `-g 3333` to the headless config instead.

Get the GDB build from
[espressif/binutils-gdb releases](https://github.com/espressif/binutils-gdb/releases).
