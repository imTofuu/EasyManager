#![feature(read_array)]
#![feature(int_roundings)]
#![feature(iter_array_chunks)]

pub mod lcd;
pub mod wifi;

use std::cell::Cell;
use std::ffi::CString;
use std::panic;
use std::panic::PanicHookInfo;
use std::rc::Rc;

use easy_manager_core::get_core_version;
use easy_manager_core::packets::CLIENT_VERSION_HN;
use embassy_executor::Spawner;
use embassy_time::Timer;
use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::Point;
use embedded_graphics::pixelcolor::{Rgb565, RgbColor};
use embedded_graphics::primitives::{Circle, PrimitiveStyle, StyledDrawable};
use embedded_sdmmc::{Mode, SdCard, VolumeIdx, VolumeManager};
use embedded_svc::http::Method;
use embedded_svc::http::client::Client;
use esp_idf_hal::spi::config::Duplex;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::delay::Delay;
use esp_idf_svc::hal::gpio::{Gpio0, Gpio1, PinDriver};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::spi;
use esp_idf_svc::hal::spi::config::DriverConfig;
use esp_idf_svc::hal::spi::{SpiDeviceDriver, SpiDriver};
use esp_idf_svc::hal::uart::config::Config;
use esp_idf_svc::hal::uart::{AsyncUartDriver, UartDriver};
use esp_idf_svc::hal::units::Hertz;
use esp_idf_svc::http::client::{Configuration as HTTPConfiguration, EspHttpConnection};
use esp_idf_svc::io::asynch::{Read, Write};
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs};
use esp_idf_svc::timer::EspTaskTimerService;
use esp_idf_svc::wifi::{AsyncWifi, EspWifi};
use log::{LevelFilter, debug, error, info};
use mipidsi::Builder;
use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;

use crate::lcd::{DummyTimesource, ReadableFile, chunked_draw_from_bitmap_reader};

fn panic(panic_info: &PanicHookInfo) {
	error!("Panicked:  {panic_info}");
	loop {}
}

use crate::lcd::SPIPinDriver;
use crate::wifi::run_wifi;

async fn uart_read_line(
	uart: &mut AsyncUartDriver<'static, UartDriver<'static>>,
	echo: bool
) -> CString {
	let mut result: Result<CString, ()> = Err(());
	while let Err(()) = result {
		let mut string = Vec::new();
		loop {
			let mut char = [0u8; 1];
			uart.read(&mut char)
				.await
				.unwrap_or_else(|err| panic!("Failed to read from uart({err})"));
			match char[0] {
				b'\n' | b'\r' => break,
				c => {
					string.push(c);
					if echo {
						uart.write(&char)
							.await
							.unwrap_or_else(|err| panic!("Failed to echo char in UART ({err})"));
					}
				}
			}
		}
		result = CString::new(string).map_err(|_| ());
	}
	result.unwrap()
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
	esp_idf_svc::sys::link_patches();

	esp_idf_svc::log::init(LevelFilter::Debug);

	panic::set_hook(Box::new(panic));

	info!("starting...");

	let peripherals =
		Peripherals::take().unwrap_or_else(|err| panic!("Failed to take peripherals ({err})"));
	let sys_loop = EspSystemEventLoop::take()
		.unwrap_or_else(|err| panic!("Failed to take event loop ({err})"));
	let nvs = EspDefaultNvsPartition::take()
		.unwrap_or_else(|err| panic!("Failed to take NVS partition ({err})"));
	let timer_service = EspTaskTimerService::new()
		.unwrap_or_else(|err| panic!("Failed to create task timer service ({err})"));

	let spi = SpiDriver::new(
		peripherals.spi2,
		peripherals.pins.gpio18,
		peripherals.pins.gpio23,
		Some(peripherals.pins.gpio19),
		&DriverConfig::default()
	)
	.unwrap_or_else(|err| panic!("Failed to create SPI driver ({err})"));

	debug!("Gotten ESP services");

	let lcd_chip_select = SPIPinDriver::new(
		PinDriver::output(peripherals.pins.gpio5)
			.unwrap_or_else(|err| panic!("Failed to create pin CS pin driver for LCD ({err})"))
	);
	let sd_card_chip_select = SPIPinDriver::new(
		PinDriver::output(peripherals.pins.gpio17)
			.unwrap_or_else(|err| panic!("Failed to create CS pin driver for SD card ({err})"))
	);
	let lcd_dc = PinDriver::output(peripherals.pins.gpio21)
		.unwrap_or_else(|err| panic!("Failed to create pin DC pin driver for LCD ({err})"));
	let lcd_rst = PinDriver::output(peripherals.pins.gpio22)
		.unwrap_or_else(|err| panic!("Failed to create pin RST pin driver for LCD ({err})"));

	let lcd_device_driver = SpiDeviceDriver::new(
		&spi,
		Some(lcd_chip_select),
		&spi::config::Config::default()
			.baudrate(Hertz(80_000_000))
			.duplex(Duplex::Half)
	)
	.unwrap_or_else(|err| panic!("Failed to create LCD SPI driver ({err})"));

	let sd_card_device_driver = SpiDeviceDriver::new(
		&spi,
		Some(sd_card_chip_select),
		&spi::config::Config::default()
			.baudrate(Hertz(8_000_000))
			.duplex(Duplex::Full)
	)
	.unwrap_or_else(|err| panic!("Failed to create SD card SPI driver ({err})"));

	let mut lcd_spi_buf = [0u8; 2048];
	let spi_interface = SpiInterface::new(lcd_device_driver, lcd_dc, &mut lcd_spi_buf);

	let mut display = Builder::new(ST7789, spi_interface)
		.reset_pin(lcd_rst)
		.init(&mut Delay::new_default())
		.unwrap_or_else(|err| panic!("Failed to create display driver ({err:?})"));

	debug!("Created display");

	let sd_card = SdCard::new(sd_card_device_driver, Delay::new_default());

	debug!("Created SD card");
	debug!(
		"SD card size: {}",
		sd_card
			.num_bytes()
			.unwrap_or_else(|err| panic!("Failed to get number of bytes in SD card ({err:?})"))
	);

	let volume_manager = VolumeManager::new(sd_card, DummyTimesource {});
	let volume = volume_manager
		.open_volume(VolumeIdx(0))
		.unwrap_or_else(|err| panic!("Failed to open volume on SD card ({err:?})"));

	let root_dir = volume
		.open_root_dir()
		.unwrap_or_else(|err| panic!("Failed to open root directory on SD card ({err:?})"));

	let onion_photo = ReadableFile::new(
		root_dir
			.open_file_in_dir("ONIONSML.BMP", Mode::ReadOnly)
			.unwrap_or_else(|err| panic!("Failed to open onion photo :( ({err:?})"))
	);

	display
		.clear(Rgb565::GREEN)
		.unwrap_or_else(|err| panic!("Failed to clear screen ({err:?})"));
	
	chunked_draw_from_bitmap_reader(&mut display, onion_photo, 5)
		.unwrap_or_else(|err| panic!("Failed to draw image ({err})"));

	Circle::with_center(Point::new(50, 50), 25)
		.draw_styled(&PrimitiveStyle::with_fill(Rgb565::WHITE), &mut display)
		.unwrap_or_else(|err| panic!("Failed to draw circle on screen ({err:?})"));

	let uart_tx_pin = peripherals.pins.gpio1;
	let uart_rx_pin = peripherals.pins.gpio3;

	let mut uart_config = Config::new();
	uart_config.baudrate = Hertz(115200);

	let uart_driver = AsyncUartDriver::new(
		peripherals.uart0,
		uart_tx_pin,
		uart_rx_pin,
		None::<Gpio0>,
		None::<Gpio1>,
		&uart_config
	)
	.unwrap_or_else(|err| panic!("Failed to create UART driver ({err})"));

	let wifi = AsyncWifi::wrap(
		EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs.clone()))
			.unwrap_or_else(|err| panic!("Failed to create inner WiFi driver ({err})")),
		sys_loop,
		timer_service
	)
	.unwrap_or_else(|err| panic!("Failed to create WiFi driver ({err})"));

	let wifi_nvs_partition = EspNvs::new(nvs, "wifi", true).expect("Failed to get NVS namespace");

	let wifi_is_connected = Rc::new(Cell::new(false));

	spawner.spawn(
		run_wifi(
			wifi,
			uart_driver,
			wifi_nvs_partition,
			wifi_is_connected.clone()
		)
		.unwrap_or_else(|err| panic!("Failed to obtain WiFi task token ({err})"))
	);

	let http_config = HTTPConfiguration {
		crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
		..Default::default()
	};

	let mut http_client =
		Client::wrap(EspHttpConnection::new(&http_config).expect("Failed to create HTTP client"));

	debug!("Created HTTP client");

	info!("Completed setup");

	loop {
		Timer::after_secs(1).await;
		if !wifi_is_connected.get() {
			continue;
		}

		let headers = [(CLIENT_VERSION_HN, get_core_version())];
		let request =
			match http_client.request(Method::Get, "https://api.easy.drewbryan.org/ping", &headers)
			{
				Ok(req) => req,
				Err(err) => {
					error!("Failed to create HTTP request ({err})");
					continue;
				}
			};

		let response = match request.submit() {
			Ok(response) => response,
			Err(err) => {
				error!("Failed to make HTTP request ({err})");
				continue;
			}
		};

		info!("Status: {}", response.status());
	}
}
