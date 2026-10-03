#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use core::sync::atomic::{AtomicU32, Ordering};
use embassy_executor::Spawner;
use embassy_net::{DhcpConfig, Runner, Stack, StackResources};
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::Level;
use esp_hal::rmt::{PulseCode, Rmt, TxChannelConfig, TxChannelCreator};
use esp_hal::rng::Rng;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_radio::wifi::{Config, Interface, WifiController, sta::StationConfig};
use log::{error, info, warn};
use picoserve::ResponseSent;
use picoserve::request::{Path, Request};
use picoserve::response::{Content, IntoResponse, ResponseWriter, StatusCode};
use picoserve::routing::PathRouterService;
use practice_esp::assets::{ASSETS, Asset};
use practice_esp::mk_static;

const SSID: &str = "Enchanter";
const PASSWORD: &str = "Khanhome";
const LED_COUNT: usize = 600;
const WS2812_BITS_PER_LED: usize = 24;
const WS2812_RESET_CODES: usize = 1;
const WS2812_FRAME_CODES: usize = LED_COUNT * WS2812_BITS_PER_LED + WS2812_RESET_CODES;
static LED_COLOR_POWER: AtomicU32 = AtomicU32::new(0x80FF_A500);
static LED_EFFECT: AtomicU32 = AtomicU32::new((128 << 8) | 0);

// Keep the large, fixed waveform buffer out of the runtime heap used for task stacks.
static mut LED_FRAME: [PulseCode; WS2812_FRAME_CODES] = [PulseCode(0); WS2812_FRAME_CODES];

/// Encode a WS2812 bit using a 40 MHz RMT clock (25 ns per tick).
fn ws2812_bit(bit: bool) -> PulseCode {
    let (high, low) = if bit { (32, 18) } else { (16, 34) };
    PulseCode::new(Level::High, high, Level::Low, low)
}

fn fill_status_frame(frame: &mut [PulseCode; 25], red: u8, green: u8, blue: u8) {
    let mut cursor = 0;
    // The onboard LED on ESP32-S3-DevKitC-1 is WS2812-compatible and uses GRB order.
    for byte in [green, red, blue] {
        for bit in (0..8).rev() {
            frame[cursor] = ws2812_bit(byte & (1 << bit) != 0);
            cursor += 1;
        }
    }
    frame[24] = PulseCode::new(Level::Low, 12_000, Level::Low, 0);
}

/// Convert a position on the 0..=255 color wheel into RGB bytes.
fn wheel(position: u8) -> (u8, u8, u8) {
    let p = position;
    match p / 85 {
        0 => (255 - p * 3, p * 3, 0),
        1 => (0, 255 - (p - 85) * 3, (p - 85) * 3),
        _ => ((p - 170) * 3, 0, 255 - (p - 170) * 3),
    }
}

fn scale(value: u8, brightness: u8) -> u8 {
    ((value as u16 * brightness as u16) / 255) as u8
}

fn fill_led_frame(frame: &mut [PulseCode], phase: u8, color_power: u32, effect: u32) {
    let mut cursor = 0;
    let powered = color_power & 0x8000_0000 != 0;
    let red = ((color_power >> 16) & 0xff) as u8;
    let green = ((color_power >> 8) & 0xff) as u8;
    let blue = (color_power & 0xff) as u8;
    let brightness = ((effect >> 8) & 0xff) as u8;
    let pattern = (effect & 0xff) as u8;

    for led in 0..LED_COUNT {
        let (mut r, mut g, mut b) = match pattern {
            1 => wheel(((led * 256 / LED_COUNT) as u8).wrapping_add(phase)),
            3 if (led + phase as usize * 3) % 24 >= 8 => (0, 0, 0),
            4 if phase % 16 < 8 => (0, 0, 0),
            5 => wheel(((led * 256 / LED_COUNT) as u8).wrapping_add(phase.wrapping_mul(2))),
            _ => (red, green, blue),
        };

        let pulse_brightness = if pattern == 2 {
            let triangle = if phase < 128 { phase } else { 255 - phase };
            (triangle as u16 * 2) as u8
        } else {
            brightness
        };
        if !powered {
            (r, g, b) = (0, 0, 0);
        }
        r = scale(r, pulse_brightness);
        g = scale(g, pulse_brightness);
        b = scale(b, pulse_brightness);

        // WS2812 LEDs receive color bytes in GRB order, most significant bit first.
        for byte in [g, r, b] {
            for bit in (0..8).rev() {
                frame[cursor] = ws2812_bit(byte & (1 << bit) != 0);
                cursor += 1;
            }
        }
    }
    // A 300 us low reset works with WS2812 variants that require a longer latch time.
    frame[LED_COUNT * WS2812_BITS_PER_LED] = PulseCode::new(Level::Low, 12_000, Level::Low, 0);
}

struct LedWebSocket;

impl picoserve::response::ws::WebSocketCallback for LedWebSocket {
    async fn run<R, W>(
        self,
        mut rx: picoserve::response::ws::SocketRx<R>,
        _tx: picoserve::response::ws::SocketTx<W>,
    ) -> Result<(), W::Error>
    where
        R: picoserve::io::Read,
        W: picoserve::io::Write<Error = R::Error>,
    {
        let mut message = [0u8; 16];
        loop {
            match rx
                .next_message(&mut message, core::future::pending::<()>())
                .await?
            {
                picoserve::futures::Either::First(Ok(
                    picoserve::response::ws::Message::Binary(data),
                )) if data.len() == 6 => {
                    let powered = data[0] != 0;
                    let color_power = ((powered as u32) << 31)
                        | ((data[1] as u32) << 16)
                        | ((data[2] as u32) << 8)
                        | data[3] as u32;
                    LED_COLOR_POWER.store(color_power, Ordering::Relaxed);
                    LED_EFFECT.store(
                        ((data[4] as u32) << 8) | data[5].min(5) as u32,
                        Ordering::Relaxed,
                    );
                }
                picoserve::futures::Either::First(Ok(picoserve::response::ws::Message::Close(
                    _,
                ))) => return Ok(()),
                picoserve::futures::Either::First(Ok(_))
                | picoserve::futures::Either::First(Err(_)) => {}
                picoserve::futures::Either::Second(()) => continue,
            }
        }
    }
}

async fn websocket_endpoint(
    upgrade: picoserve::response::ws::WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade.on_upgrade(LedWebSocket)
}

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", panic_info);
    loop {}
}

// This creates a default app-descriptor required by the esp-idf bootloader.
esp_bootloader_esp_idf::esp_app_desc!();

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    loop {
        while controller.ap_info().is_ok() {
            Timer::after(Duration::from_secs(1)).await;
        }

        let station_config = Config::Station(
            StationConfig::default()
                .with_ssid(SSID)
                .with_password(PASSWORD.into()),
        );

        controller
            .set_config(&station_config)
            .expect("failed to configure Wi-Fi station");
        info!("Wi-Fi station configured");

        match controller.connect_async().await {
            Ok(info) => info!(
                "Connected to access point '{}' on channel {}",
                SSID, info.channel
            ),
            Err(err) => {
                warn!("Wi-Fi connect failed: {:?}", err);
                Timer::after(Duration::from_secs(5)).await;
            }
        }
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface<'static>>) {
    runner.run().await
}

#[embassy_executor::task]
async fn http_server(stack: Stack<'static>) {
    let app = picoserve::Router::from_service(EmbeddedAssets)
        .route("/ws", picoserve::routing::get(websocket_endpoint));
    let config = picoserve::Config::new(picoserve::Timeouts {
        start_read_request: Some(Duration::from_secs(5)),
        persistent_start_read_request: Some(Duration::from_secs(1)),
        read_request: Some(Duration::from_secs(1)),
        write: Some(Duration::from_secs(5)),
    });

    loop {
        picoserve::Server::new(&app, &config, &mut [0; 1024])
            .listen_and_serve("web", stack, 80, &mut [0; 4096], &mut [0; 4096])
            .await;
    }
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    esp_alloc::heap_allocator!(size: 98_767);

    let rmt = Rmt::new(peripherals.RMT, Rate::from_mhz(80)).expect("failed to initialize RMT");
    let tx_config = TxChannelConfig::default().with_clk_divider(2);
    let mut led_channel = rmt
        .channel0
        .configure_tx(&tx_config)
        .expect("failed to configure RMT TX")
        .with_pin(peripherals.GPIO4);
    // GPIO 48 drives the built-in RGB LED on ESP32-S3-DevKitC-1 boards.
    let mut status_channel = rmt
        .channel1
        .configure_tx(&tx_config)
        .expect("failed to configure status LED RMT TX")
        .with_pin(peripherals.GPIO48);
    let mut status_frame = [PulseCode::default(); 25];
    fill_status_frame(&mut status_frame, 0, 0, 0);
    status_channel = status_channel
        .transmit(&status_frame)
        .expect("failed to turn off status LED")
        .wait()
        .expect("status LED off transmission failed");

    // SAFETY: main is the only code that accesses this buffer, and it retains
    // exclusive access for the lifetime of the LED transmission loop.
    let led_frame = unsafe { &mut *core::ptr::addr_of_mut!(LED_FRAME) };

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    let (wifi_controller, interfaces) = esp_radio::wifi::new(peripherals.WIFI, Default::default())
        .expect("failed to initialize Wi-Fi");

    let rng = Rng::new();
    let net_seed = rng.random() as u64 | ((rng.random() as u64) << 32);
    let dhcp_config = DhcpConfig::default();
    let net_config = embassy_net::Config::dhcpv4(dhcp_config);

    let (stack, runner) = embassy_net::new(
        interfaces.station,
        net_config,
        mk_static!(StackResources<4>, StackResources::<4>::new()),
        net_seed,
    );

    spawner.spawn(connection(wifi_controller).expect("failed to create Wi-Fi connection task"));
    spawner.spawn(net_task(runner).expect("failed to create network task"));

    wait_for_network(stack).await;
    spawner.spawn(http_server(stack).expect("failed to create HTTP server task"));
    fill_status_frame(&mut status_frame, 0, 24, 0);
    status_channel
        .transmit(&status_frame)
        .expect("failed to turn on status LED")
        .wait()
        .expect("status LED on transmission failed");

    let mut phase = 0u8;
    loop {
        fill_led_frame(
            led_frame,
            phase,
            LED_COLOR_POWER.load(Ordering::Relaxed),
            LED_EFFECT.load(Ordering::Relaxed),
        );
        let transaction = led_channel
            .transmit(led_frame)
            .expect("failed to start WS2812 frame");
        led_channel = transaction.wait().expect("WS2812 transmission failed");
        phase = phase.wrapping_add(1);
        Timer::after(Duration::from_millis(30)).await;
    }
}

async fn wait_for_network(stack: Stack<'_>) {
    info!("Waiting for Wi-Fi link");
    while !stack.is_link_up() {
        Timer::after(Duration::from_millis(500)).await;
    }

    info!("Waiting for DHCP lease");
    loop {
        if let Some(config) = stack.config_v4() {
            info!("Serving React app at http://{}/", config.address.address());
            break;
        }

        Timer::after(Duration::from_millis(500)).await;
    }
}

fn find_asset(path: &str) -> Option<&'static Asset> {
    let normalized = if path == "/" { "/index.html" } else { path };

    ASSETS
        .iter()
        .find(|asset| asset.path == normalized)
        .or_else(|| {
            if normalized.rsplit('/').next().unwrap_or("").contains('.') {
                None
            } else {
                ASSETS.iter().find(|asset| asset.path == "/index.html")
            }
        })
}

struct EmbeddedAssets;

impl<State> PathRouterService<State> for EmbeddedAssets {
    async fn call_request_handler_service<R, W>(
        &self,
        _state: &State,
        _path_parameters: (),
        path: Path<'_>,
        request: Request<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error>
    where
        R: picoserve::io::Read,
        W: ResponseWriter<Error = R::Error>,
    {
        match request.parts.method() {
            "GET" | "HEAD" => {
                let asset = find_asset(path.encoded()).unwrap_or_else(|| {
                    // Fallback to index.html for SPA routing
                    ASSETS
                        .iter()
                        .find(|a| a.path == "/index.html")
                        .expect("index.html must exist")
                });

                (
                    ("Content-Encoding", "gzip"),
                    ("Vary", "Accept-Encoding"),
                    ("Cache-Control", "public, max-age=31536000, immutable"),
                    EncodedAsset(asset),
                )
                    .write_to(request.body_connection.finalize().await?, response_writer)
                    .await
            }
            _ => {
                (StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed")
                    .write_to(request.body_connection.finalize().await?, response_writer)
                    .await
            }
        }
    }
}

struct EncodedAsset(&'static Asset);

impl Content for EncodedAsset {
    fn content_type(&self) -> &'static str {
        self.0.content_type
    }

    fn content_length(&self) -> usize {
        self.0.bytes.len()
    }

    async fn write_content<W: picoserve::io::Write>(self, mut writer: W) -> Result<(), W::Error> {
        writer.write_all(self.0.bytes).await
    }
}
