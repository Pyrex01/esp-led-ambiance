#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use embassy_executor::Spawner;
use embassy_net::{DhcpConfig, Runner, Stack, StackResources};
use embassy_time::{Duration, Timer};
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::rmt::{PulseCode, Rmt, TxChannelConfig, TxChannelCreator};
use esp_hal::rng::Rng;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
#[cfg(feature = "wokwi")]
use esp_radio::wifi::AuthenticationMethod;
use esp_radio::wifi::{Config, Interface, WifiController, sta::StationConfig};
use esp_storage::FlashStorage;
use log::{error, info, warn};
use picoserve::ResponseSent;
use picoserve::request::{Path, Request};
use picoserve::response::{Content, IntoResponse, ResponseWriter, StatusCode};
use picoserve::routing::PathRouterService;
use practice_esp::assets::{ASSETS, Asset};
use practice_esp::mk_static;

#[cfg(not(feature = "wokwi"))]
const SSID: &str = "Enchanter";
#[cfg(not(feature = "wokwi"))]
const PASSWORD: &str = "Khanhome";
// The Wokwi simulator provides an open access point on channel 6.
#[cfg(feature = "wokwi")]
const SSID: &str = "Wokwi-GUEST";
#[cfg(feature = "wokwi")]
const PASSWORD: &str = "";
const LEDS_PER_SIDE: usize = 120;
const LED_COUNT: usize = LEDS_PER_SIDE * 4;
const LED_MASK_WORDS: usize = LED_COUNT / 32;
const WS2812_BITS_PER_LED: usize = 24;
const WS2812_RESET_CODES: usize = 1;
const WS2812_FRAME_CODES: usize = LED_COUNT * WS2812_BITS_PER_LED + WS2812_RESET_CODES;
const SETTINGS_PARTITION_START: u32 = 0x3F_E000;
const SETTINGS_SLOT_SIZE: u32 = 0x1000;
const SETTINGS_MAGIC: u32 = 0x4C45_4453;
const SETTINGS_RECORD_SIZE: usize = 80;
const OTA_CHUNK_SIZE: usize = 4096;
static OTA_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static LED_COLOR_POWER: AtomicU32 = AtomicU32::new(0x80FF_A500);
static LED_EFFECT: AtomicU32 = AtomicU32::new((128 << 8) | 0);
static AMBIENT_ACTIVE: AtomicBool = AtomicBool::new(false);
static AMBIENT_COLORS: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
static LED_MASK: [AtomicU32; LED_MASK_WORDS] = [const { AtomicU32::new(u32::MAX) }; LED_MASK_WORDS];
static WIFI_CONNECT_FAILED: AtomicBool = AtomicBool::new(false);

// Keep the large, fixed waveform buffer out of the runtime heap used for task stacks.
static mut LED_FRAME: [PulseCode; WS2812_FRAME_CODES] = [PulseCode(0); WS2812_FRAME_CODES];

#[derive(Clone, Copy, PartialEq, Eq)]
struct LedState {
    color_power: u32,
    effect: u32,
    mask: [u32; LED_MASK_WORDS],
}

struct StateStore {
    flash: FlashStorage,
    active_slot: Option<u8>,
    sequence: u32,
}

impl LedState {
    fn current() -> Self {
        Self {
            color_power: LED_COLOR_POWER.load(Ordering::Relaxed),
            effect: LED_EFFECT.load(Ordering::Relaxed),
            mask: core::array::from_fn(|index| LED_MASK[index].load(Ordering::Relaxed)),
        }
    }

    fn to_packet(self) -> [u8; 6 + LED_COUNT / 8] {
        let color_power = self.color_power;
        let effect = self.effect;
        let mut packet = [0u8; 6 + LED_COUNT / 8];
        packet[0] = (color_power >> 31) as u8;
        packet[1] = (color_power >> 16) as u8;
        packet[2] = (color_power >> 8) as u8;
        packet[3] = color_power as u8;
        packet[4] = (effect >> 8) as u8;
        packet[5] = effect as u8;
        for (index, byte) in packet[6..].iter_mut().enumerate() {
            let word = self.mask[index / 4].to_le_bytes();
            *byte = word[index % 4];
        }
        packet
    }

    fn restore(self) {
        LED_COLOR_POWER.store(self.color_power, Ordering::Relaxed);
        LED_EFFECT.store(self.effect, Ordering::Relaxed);
        for (word, value) in self.mask.into_iter().enumerate() {
            LED_MASK[word].store(value, Ordering::Relaxed);
        }
    }

    fn encode(self, sequence: u32) -> [u8; SETTINGS_RECORD_SIZE] {
        let mut bytes = [0u8; SETTINGS_RECORD_SIZE];
        bytes[0..4].copy_from_slice(&SETTINGS_MAGIC.to_le_bytes());
        bytes[4..8].copy_from_slice(&sequence.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.color_power.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.effect.to_le_bytes());
        for (index, word) in self.mask.iter().enumerate() {
            let offset = 16 + index * 4;
            bytes[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
        }
        let checksum = crc32(&bytes[..76]);
        bytes[76..80].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    fn decode(bytes: &[u8; SETTINGS_RECORD_SIZE]) -> Option<(Self, u32)> {
        if u32::from_le_bytes(bytes[0..4].try_into().ok()?) != SETTINGS_MAGIC
            || u32::from_le_bytes(bytes[76..80].try_into().ok()?) != crc32(&bytes[..76])
        {
            return None;
        }
        let mut mask = [0; LED_MASK_WORDS];
        for (index, word) in mask.iter_mut().enumerate() {
            let offset = 16 + index * 4;
            *word = u32::from_le_bytes(bytes[offset..offset + 4].try_into().ok()?);
        }
        Some((
            Self {
                color_power: u32::from_le_bytes(bytes[8..12].try_into().ok()?),
                effect: u32::from_le_bytes(bytes[12..16].try_into().ok()?),
                mask,
            },
            u32::from_le_bytes(bytes[4..8].try_into().ok()?),
        ))
    }
}

impl StateStore {
    fn load() -> (Self, Option<LedState>) {
        let mut store = Self {
            flash: FlashStorage::new(),
            active_slot: None,
            sequence: 0,
        };
        let mut best: Option<(LedState, u32, u8)> = None;
        for slot in 0..2 {
            let mut bytes = [0u8; SETTINGS_RECORD_SIZE];
            let offset = SETTINGS_PARTITION_START + slot as u32 * SETTINGS_SLOT_SIZE;
            if store.flash.read(offset, &mut bytes).is_ok() {
                if let Some((state, sequence)) = LedState::decode(&bytes) {
                    if best.as_ref().is_none_or(|(_, latest, _)| {
                        sequence.wrapping_sub(*latest) < 0x8000_0000 && sequence != *latest
                    }) {
                        best = Some((state, sequence, slot));
                    }
                }
            }
        }
        let state = best.map(|(state, sequence, slot)| {
            store.sequence = sequence;
            store.active_slot = Some(slot);
            state
        });
        (store, state)
    }

    fn save(&mut self, state: LedState) -> Result<(), esp_storage::FlashStorageError> {
        let next_slot = self.active_slot.map_or(0, |slot| 1 - slot);
        let start = SETTINGS_PARTITION_START + next_slot as u32 * SETTINGS_SLOT_SIZE;
        self.flash.erase(start, start + SETTINGS_SLOT_SIZE)?;
        self.sequence = self.sequence.wrapping_add(1);
        let record = state.encode(self.sequence);
        self.flash.write(start, &record)?;
        self.active_slot = Some(next_slot);
        Ok(())
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

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
    let ambient = AMBIENT_ACTIVE.load(Ordering::Relaxed);

    for led in 0..LED_COUNT {
        let (mut r, mut g, mut b) = if ambient {
            let side = led / LEDS_PER_SIDE;
            let color = AMBIENT_COLORS[side].load(Ordering::Relaxed);
            (
                ((color >> 16) & 0xff) as u8,
                ((color >> 8) & 0xff) as u8,
                (color & 0xff) as u8,
            )
        } else {
            match pattern {
                1 => wheel(((led * 256 / LED_COUNT) as u8).wrapping_add(phase)),
                3 if (led + phase as usize * 3) % 24 >= 8 => (0, 0, 0),
                4 if phase % 16 < 8 => (0, 0, 0),
                5 => wheel(((led * 256 / LED_COUNT) as u8).wrapping_add(phase.wrapping_mul(2))),
                _ => (red, green, blue),
            }
        };

        let pulse_brightness = if ambient {
            brightness
        } else if pattern == 2 {
            let triangle = if phase < 128 { phase } else { 255 - phase };
            (triangle as u16 * 2) as u8
        } else {
            brightness
        };
        // Ambient mode represents the display edge across every physical LED.
        // The manual per-LED mask is for setup/effects and may contain saved
        // exclusions, which should not leave gaps in the ambient output.
        let led_on = ambient
            || LED_MASK[led / 32].load(Ordering::Relaxed) & (1 << (led % 32)) != 0;
        if !powered || !led_on {
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
        mut tx: picoserve::response::ws::SocketTx<W>,
    ) -> Result<(), W::Error>
    where
        R: picoserve::io::Read,
        W: picoserve::io::Write<Error = R::Error>,
    {
        tx.send_binary(&LedState::current().to_packet()).await?;
        let mut message = [0u8; 66];
        let mut ambient_ack_sent = false;
        loop {
            match rx
                .next_message(&mut message, core::future::pending::<()>())
                .await?
            {
                picoserve::futures::Either::First(Ok(
                    picoserve::response::ws::Message::Binary(data),
                )) if data.len() == 14 && data[0] == 0xB1 => {
                    AMBIENT_ACTIVE.store(true, Ordering::Relaxed);
                    LED_COLOR_POWER.store(((data[1] & 1 != 0) as u32) << 31, Ordering::Relaxed);
                    LED_EFFECT.store((data[2] as u32) << 8, Ordering::Relaxed);
                    for side in 0..4 {
                        let offset = 3 + side * 3;
                        AMBIENT_COLORS[side].store(
                            ((data[offset] as u32) << 16) | ((data[offset + 1] as u32) << 8) | data[offset + 2] as u32,
                            Ordering::Relaxed,
                        );
                    }
                    // Confirm the stream reached firmware without adding an
                    // acknowledgement for every 30 Hz color update.
                    if !ambient_ack_sent {
                        tx.send_binary(&[0xA1, 1]).await?;
                        ambient_ack_sent = true;
                    }
                }
                picoserve::futures::Either::First(Ok(
                    picoserve::response::ws::Message::Binary(data),
                )) if data.len() == 6 + LED_COUNT / 8 => {
                    AMBIENT_ACTIVE.store(false, Ordering::Relaxed);
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
                    for word in 0..LED_MASK_WORDS {
                        let offset = 6 + word * 4;
                        let bits = u32::from_le_bytes([
                            data[offset],
                            data[offset + 1],
                            data[offset + 2],
                            data[offset + 3],
                        ]);
                        LED_MASK[word].store(bits, Ordering::Relaxed);
                    }
                }
                picoserve::futures::Either::First(Ok(picoserve::response::ws::Message::Close(_))) => {
                    if AMBIENT_ACTIVE.load(Ordering::Relaxed) {
                        AMBIENT_ACTIVE.store(false, Ordering::Relaxed);
                        LED_COLOR_POWER.store(0, Ordering::Relaxed);
                    }
                    return Ok(());
                }
                picoserve::futures::Either::First(Ok(_)) => {}
                picoserve::futures::Either::First(Err(_)) => {
                    if AMBIENT_ACTIVE.load(Ordering::Relaxed) {
                        AMBIENT_ACTIVE.store(false, Ordering::Relaxed);
                        LED_COLOR_POWER.store(0, Ordering::Relaxed);
                    }
                    return Ok(());
                }
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

        WIFI_CONNECT_FAILED.store(false, Ordering::Relaxed);

        let station_config = StationConfig::default()
            .with_ssid(SSID)
            .with_password(PASSWORD.into());
        #[cfg(feature = "wokwi")]
        let station_config = station_config
            .with_auth_method(AuthenticationMethod::None)
            .with_channel(6);
        let station_config = Config::Station(station_config);

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
                WIFI_CONNECT_FAILED.store(true, Ordering::Relaxed);
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

#[embassy_executor::task(pool_size = 3)]
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
            .listen_and_serve("web", stack, 80, &mut [0; 2048], &mut [0; 2048])
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

    let (mut state_store, restored_state) = StateStore::load();
    if let Some(state) = restored_state {
        state.restore();
        info!("Restored LED settings from flash");
    } else {
        info!("No saved LED settings found; using defaults");
    }

    // GPIO 5 follows the power state selected in the web UI.
    let mut power_output = Output::new(peripherals.GPIO5, Level::Low, OutputConfig::default());

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
        mk_static!(StackResources<8>, StackResources::<8>::new()),
        net_seed,
    );

    spawner.spawn(connection(wifi_controller).expect("failed to create Wi-Fi connection task"));
    spawner.spawn(net_task(runner).expect("failed to create network task"));

    status_channel = wait_for_network(stack, status_channel, &mut status_frame).await;
    spawner.spawn(http_server(stack).expect("failed to create HTTP server task"));
    // Each picoserve task handles one connection at a time. Run three listeners
    // so multiple browsers and their WebSockets can stay connected concurrently.
    spawner.spawn(http_server(stack).expect("failed to create HTTP server task"));
    spawner.spawn(http_server(stack).expect("failed to create HTTP server task"));
    // Keep the onboard indicator off while the 480-pixel strip is running.
    fill_status_frame(&mut status_frame, 0, 0, 0);
    status_channel
        .transmit(&status_frame)
        .expect("failed to turn off status LED")
        .wait()
        .expect("status LED off transmission failed");

    let mut phase = 0u8;
    let mut observed_state = LedState::current();
    let mut persisted_state = restored_state.unwrap_or(observed_state);
    let mut unsaved_for_ms = 0u32;
    loop {
        if LED_COLOR_POWER.load(Ordering::Relaxed) & 0x8000_0000 != 0 {
            power_output.set_high();
        } else {
            power_output.set_low();
        }
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
        let current_state = LedState::current();
        if current_state != observed_state {
            observed_state = current_state;
            unsaved_for_ms = 0;
        }
        if current_state != persisted_state {
            unsaved_for_ms = unsaved_for_ms.saturating_add(30);
            if unsaved_for_ms >= 500 {
                match state_store.save(current_state) {
                    Ok(()) => {
                        persisted_state = current_state;
                        info!("Saved LED settings to flash");
                    }
                    Err(err) => warn!("Could not save LED settings: {:?}", err),
                }
                unsaved_for_ms = 0;
            }
        } else {
            unsaved_for_ms = 0;
        }
        Timer::after(Duration::from_millis(30)).await;
    }
}

async fn wait_for_network<'stack, 'channel, 'frame>(
    stack: Stack<'stack>,
    mut status_channel: esp_hal::rmt::Channel<'channel, esp_hal::Blocking, esp_hal::rmt::Tx>,
    status_frame: &'frame mut [PulseCode; 25],
) -> esp_hal::rmt::Channel<'channel, esp_hal::Blocking, esp_hal::rmt::Tx> {
    info!("Waiting for Wi-Fi link");
    let mut blink_on = false;
    while !stack.is_link_up() {
        if WIFI_CONNECT_FAILED.load(Ordering::Relaxed) {
            fill_status_frame(status_frame, 24, 0, 0);
        } else {
            blink_on = !blink_on;
            if blink_on {
                fill_status_frame(status_frame, 0, 0, 24);
            } else {
                fill_status_frame(status_frame, 0, 0, 0);
            }
        }
        status_channel = status_channel
            .transmit(status_frame)
            .expect("failed to update Wi-Fi status LED")
            .wait()
            .expect("Wi-Fi status LED transmission failed");
        Timer::after(Duration::from_millis(500)).await;
    }

    info!("Waiting for DHCP lease");
    loop {
        if let Some(config) = stack.config_v4() {
            info!("Serving React app at http://{}/", config.address.address());
            return status_channel;
        }

        if WIFI_CONNECT_FAILED.load(Ordering::Relaxed) {
            fill_status_frame(status_frame, 24, 0, 0);
        } else {
            blink_on = !blink_on;
            if blink_on {
                fill_status_frame(status_frame, 0, 0, 24);
            } else {
                fill_status_frame(status_frame, 0, 0, 0);
            }
        }
        status_channel = status_channel
            .transmit(status_frame)
            .expect("failed to update Wi-Fi status LED")
            .wait()
            .expect("Wi-Fi status LED transmission failed");
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
        if request.parts.method() == "POST" && path.encoded() == "/update" {
            return handle_firmware_update(request, response_writer).await;
        }

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

async fn handle_firmware_update<R, W>(
    mut request: Request<'_, R>,
    response_writer: W,
) -> Result<ResponseSent, W::Error>
where
    R: picoserve::io::Read,
    W: ResponseWriter<Error = R::Error>,
{
    let body = request.body_connection.body();
    let image_len = body.content_length();
    if image_len < 24 || image_len > 0x1F_0000 {
        return (StatusCode::BAD_REQUEST, "Invalid firmware image size")
            .write_to(request.body_connection.finalize().await?, response_writer)
            .await;
    }
    if OTA_IN_PROGRESS.swap(true, Ordering::AcqRel) {
        return (StatusCode::SERVICE_UNAVAILABLE, "An update is already running")
            .write_to(request.body_connection.finalize().await?, response_writer)
            .await;
    }

    // Stream and align writes to the flash driver's 4-byte write requirement.
    let mut reader = body.reader();
    let mut flash = FlashStorage::new();
    let mut partition_table = [0u8; esp_bootloader_esp_idf::partitions::PARTITION_TABLE_MAX_LEN];
    let result = async {
        let mut updater = esp_bootloader_esp_idf::ota_updater::OtaUpdater::new(
            &mut flash,
            &mut partition_table,
        )
        .map_err(|_| ())?;
        {
            let (mut partition, _) = updater.next_partition().map_err(|_| ())?;
            if image_len > partition.partition_size() {
                return Err(());
            }
            let erase_len = (image_len + 0xFFF) & !0xFFF;
            partition.erase(0, erase_len as u32).map_err(|_| ())?;

            let mut buffer = [0xFFu8; OTA_CHUNK_SIZE];
            let mut offset = 0usize;
            let mut first = true;
            while offset < image_len {
                let count = (image_len - offset).min(OTA_CHUNK_SIZE);
                let mut filled = 0;
                while filled < count {
                    let read = futures_read(&mut reader, &mut buffer[filled..count]).await?;
                    if read == 0 {
                        return Err(());
                    }
                    filled += read;
                }
                if first && buffer[0] != 0xE9 {
                    return Err(());
                }
                first = false;
                let write_len = (count + 3) & !3;
                partition.write(offset as u32, &buffer[..write_len]).map_err(|_| ())?;
                offset += count;
                buffer.fill(0xFF);
            }
        }
        updater.activate_next_partition().map_err(|_| ())?;
        Ok(())
    }
    .await;

    // Ensure the body is consumed and release the connection before sending a response.
    let connection = request.body_connection.finalize().await?;
    OTA_IN_PROGRESS.store(false, Ordering::Release);
    if result.is_ok() {
        let _response = (StatusCode::OK, "Firmware installed; restarting now.")
            .write_to(connection, response_writer)
            .await?;
        Timer::after(Duration::from_millis(300)).await;
        esp_hal::system::software_reset();
        #[allow(unreachable_code)]
        Ok(_response)
    } else {
        (StatusCode::BAD_REQUEST, "Firmware update failed")
            .write_to(connection, response_writer)
            .await
    }
}

async fn futures_read<R: picoserve::io::Read>(
    reader: &mut picoserve::request::RequestBodyReader<'_, R>,
    buffer: &mut [u8],
) -> Result<usize, ()> {
    use embedded_io_async::Read as _;
    reader.read(buffer).await.map_err(|_| ())
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
