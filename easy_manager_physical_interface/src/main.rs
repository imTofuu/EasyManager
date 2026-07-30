#![feature(read_array)]
#![feature(int_roundings)]
#![feature(iter_array_chunks)]
#![feature(fn_traits)]
#![feature(unsafe_cell_access)]
#![feature(push_mut)]
#![feature(ptr_as_ref_unchecked)]
#![warn(clippy::unwrap_used)]
#![allow(clippy::mutex)]

mod communications;
mod graphics;
pub mod lcd;
mod pages;

use std::cell::Cell;
use std::panic;
use std::panic::PanicHookInfo;
use std::rc::Rc;

use cstr_core::CString;
use embassy_executor::Spawner;
use embassy_time::Timer;
use esp_idf_hal::rmt::config::{MemoryAccess, TxChannelConfig};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::gpio::{Gpio0, Gpio1};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::spi::SpiDriver;
use esp_idf_svc::hal::spi::config::DriverConfig;
use esp_idf_svc::hal::uart::config::Config;
use esp_idf_svc::hal::uart::{AsyncUartDriver, UartDriver};
use esp_idf_svc::hal::units::Hertz;
use esp_idf_svc::http::client::Configuration as HTTPConfiguration;
use esp_idf_svc::io::asynch::{Read, Write};
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs};
use esp_idf_svc::timer::EspTaskTimerService;
use esp_idf_svc::wifi::{AsyncWifi, EspWifi};
use log::{LevelFilter, debug, error, info};
use rgb::RGB8;
use rustyfarian_esp_idf_ws2812::Ws2812Rmt;

use crate::communications::{RfidReader, run_wifi};
use crate::lcd::run_lcd;

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

	let led_channel_config = TxChannelConfig {
		resolution: Hertz(10_000_000),
		memory_access: MemoryAccess::Indirect {
			memory_block_symbols: 64
		},
		..Default::default()
	};

	let mut led_driver =
		Ws2812Rmt::new_with_channel_config(peripherals.pins.gpio25, led_channel_config)
			.expect("Failed to create LED driver");
	led_driver
		.set_pixel(RGB8::new(0 /* green */, 255, 0))
		.unwrap();

	let spi2 = SpiDriver::new(
		peripherals.spi2,
		peripherals.pins.gpio14,
		peripherals.pins.gpio13,
		None::<Gpio0>,
		&DriverConfig::default()
	)
	.unwrap_or_else(|err| panic!("Failed to create SPI driver ({err})"));

	let spi3 = SpiDriver::new(
		peripherals.spi3,
		peripherals.pins.gpio18,
		peripherals.pins.gpio23,
		Some(peripherals.pins.gpio19),
		&DriverConfig::default()
	)
	.unwrap_or_else(|err| panic!("Failed to create SPI driver ({err})"));

	debug!("Gotten ESP services");

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

	let http_config = HTTPConfiguration {
		crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
		..Default::default()
	};

	let mut rfid_device = RfidReader::new(&spi3, Some(peripherals.pins.gpio5), |uid, _| {
		info!("uid: {:?}", uid.as_bytes());
		[0xff; 6]
	})
	.unwrap_or_else(|err| panic!("Failed to create RFID device ({err})"));

	spawner.spawn(
		run_lcd(
			http_config,
			spi2,
			peripherals.pins.gpio15.degrade_output(),
			peripherals.pins.gpio26.degrade_output(),
			peripherals.pins.gpio27.degrade_output(),
			peripherals.pins.gpio16.degrade_input(),
			peripherals.pins.gpio17.degrade_input(),
			peripherals.pins.gpio21.degrade_input()
		)
		.unwrap_or_else(|err| panic!("Failed to obtain LCD task token ({err})"))
	);

	spawner.spawn(
		run_wifi(
			wifi,
			uart_driver,
			wifi_nvs_partition,
			wifi_is_connected.clone()
		)
		.unwrap_or_else(|err| panic!("Failed to obtain WiFi task token ({err})"))
	);
	info!("Completed setup");

	loop {
		Timer::after_millis(5).await;

		match rfid_device.reqa_collect_data(3..5) {
			Ok(Some(data)) => {
				info!("rfid data: {data:?}")
			}
			Err(err) => {
				error!("Failed to read RFID tag: ({err})");
			}
			_ => {}
		}

		if let Err(err) = rfid_device.reqa_write_data([(1u8, *b"hello\0\0\0\0\0\0\0\0\0\0\0")]) {
			error!("Failed to write RFID tag ({err})");
		}

		/*if !wifi_is_connected.get() {
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

		info!("Status: {}", response.status());*/
	}
}
