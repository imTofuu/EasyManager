#![feature(read_array)]
#![feature(int_roundings)]
#![feature(iter_array_chunks)]

pub mod lcd;
pub mod wifi;

use std::cell::Cell;
use std::panic;
use std::panic::PanicHookInfo;
use std::rc::Rc;

use cstr_core::CString;
use easy_manager_core::get_core_version;
use easy_manager_core::packets::CLIENT_VERSION_HN;
use embassy_executor::Spawner;
use embassy_time::Timer;
use embedded_svc::http::Method;
use embedded_svc::http::client::Client;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::gpio::{Gpio0, Gpio1};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::spi::SpiDriver;
use esp_idf_svc::hal::spi::config::DriverConfig;
use esp_idf_svc::hal::uart::config::Config;
use esp_idf_svc::hal::uart::{AsyncUartDriver, UartDriver};
use esp_idf_svc::hal::units::Hertz;
use esp_idf_svc::http::client::{Configuration as HTTPConfiguration, EspHttpConnection};
use esp_idf_svc::io::asynch::{Read, Write};
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs};
use esp_idf_svc::timer::EspTaskTimerService;
use esp_idf_svc::wifi::{AsyncWifi, EspWifi};
use log::{LevelFilter, debug, error, info};

use crate::lcd::{run_lcd, setup_lcd};
use crate::wifi::run_wifi;

fn panic(panic_info: &PanicHookInfo) {
	error!("Panicked:  {panic_info}");
	loop {}
}

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
	lvgl::init();
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

	// Drop these so they aren't accidentally used after giving them to the lcd
	peripherals.pins.gpio5;
	peripherals.pins.gpio21;
	peripherals.pins.gpio22;

	let _display =
		setup_lcd(&spi, 5, 21, 22).unwrap_or_else(|err| panic!("Failed to setup LCD ({err})"));

	debug!("Created display");

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

	spawner
		.spawn(run_lcd().unwrap_or_else(|err| panic!("Failed to obtain LCD task token ({err})")));

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
		Timer::after_millis(50).await;

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
