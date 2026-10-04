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

## Run the web app on a development computer

Connect the computer and ESP32 to the same Wi-Fi network, then run Vite with
the ESP32's IP address. Vite proxies the page's `/ws` connection to the device:

```sh
cd web
ESP32_HOST=http://192.168.1.42 yarn dev
```

Replace `192.168.1.42` with the address printed by the ESP32 serial monitor.
The default proxy target is `http://192.168.4.1`.

## Build and flash firmware

The Wi-Fi access point name and password are set in `src/bin/main.rs`:

```rust
const SSID: &str = "Enchanter";
const PASSWORD: &str = "Khanhome";
```

Flash the firmware:

```sh
cargo run --release
```

The serial monitor prints the assigned URL, for example:

```text
Serving React app at http://192.168.1.42/
```

The firmware saves the LED power, color, effect, and per-LED mask to the
`led_state` flash partition after changes settle, then restores them at boot.
`espflash.toml` selects the custom 4 MB partition table in `partitions.bin`;
keep that table and `partitions.csv` together when flashing the firmware.

Browsers request Brotli by default, and the ESP replies with
`Content-Encoding: br`.
