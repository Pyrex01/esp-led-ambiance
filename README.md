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

Browsers request Brotli by default, and the ESP replies with
`Content-Encoding: br`.
