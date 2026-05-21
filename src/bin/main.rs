#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_net::{DhcpConfig, Runner, Stack, StackResources};
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::rng::Rng;
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
    let app = picoserve::Router::from_service(EmbeddedAssets);
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

    loop {
        Timer::after(Duration::from_secs(60)).await;
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
                    ("Content-Encoding", "br"),
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
