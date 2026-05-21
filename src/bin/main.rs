#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use core::fmt::Write as _;
use core::str;

use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_net::{DhcpConfig, Runner, Stack, StackResources};
use embassy_time::{Duration, Timer};
use embedded_io_async::Write as _;
use esp_hal::clock::CpuClock;
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use esp_radio::wifi::{sta::StationConfig, Config, Interface, WifiController};
use heapless::String;
use log::{error, info, warn};
use practice_esp::assets::{Asset, ASSETS};
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
    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];
    let mut request = [0; 1024];

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);

        if let Err(err) = socket.accept(80).await {
            warn!("HTTP accept failed: {:?}", err);
            Timer::after(Duration::from_millis(250)).await;
            continue;
        }

        let read = match socket.read(&mut request).await {
            Ok(read) if read > 0 => read,
            Ok(_) => {
                socket.close();
                continue;
            }
            Err(err) => {
                warn!("HTTP read failed: {:?}", err);
                socket.close();
                continue;
            }
        };

        let path = request_path(&request[..read]).unwrap_or("/");
        let response = find_asset(path);

        let result = match response {
            Some(asset) => write_asset(&mut socket, asset).await,
            None => write_404(&mut socket).await,
        };

        if let Err(err) = result {
            warn!("HTTP write failed: {:?}", err);
        }

        socket.close();
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

fn request_path(request: &[u8]) -> Option<&str> {
    let line_end = request.windows(2).position(|window| window == b"\r\n")?;
    let line = str::from_utf8(&request[..line_end]).ok()?;
    let mut parts = line.split_ascii_whitespace();
    let method = parts.next()?;
    let path = parts.next()?;

    if method != "GET" && method != "HEAD" {
        return None;
    }

    Some(path.split('?').next().unwrap_or("/"))
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

async fn write_asset(
    socket: &mut TcpSocket<'_>,
    asset: &Asset,
) -> Result<(), embassy_net::tcp::Error> {
    let mut header: String<256> = String::new();
    write!(
        header,
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Encoding: br\r\nVary: Accept-Encoding\r\nCache-Control: public, max-age=31536000, immutable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        asset.content_type,
        asset.bytes.len()
    )
    .ok();

    socket.write_all(header.as_bytes()).await?;
    socket.write_all(asset.bytes).await
}

async fn write_404(socket: &mut TcpSocket<'_>) -> Result<(), embassy_net::tcp::Error> {
    const BODY: &[u8] = b"Not Found";
    let mut header: String<128> = String::new();
    write!(
        header,
        "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        BODY.len()
    )
    .ok();

    socket.write_all(header.as_bytes()).await?;
    socket.write_all(BODY).await
}
