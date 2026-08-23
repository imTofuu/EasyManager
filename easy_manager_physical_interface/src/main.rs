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
use esp_idf_hal::gpio::PinDriver;
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

use crate::communications::run_wifi;
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
	// These need to be called for esp_idf
	esp_idf_svc::sys::link_patches();
	esp_idf_svc::log::init(LevelFilter::Debug);

	// This is optional, but I use it to add a delay before the reboot (delay is
	// watchdog timeout time)
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

	// This is just debug lights for now, they don't do anything yet
	let led_channel_config = TxChannelConfig {
		resolution: Hertz(10_000_000),
		memory_access: MemoryAccess::Indirect {
			memory_block_symbols: 64
		},
		..Default::default()
	};

	let mut led_driver =
		Ws2812Rmt::new_with_channel_config(peripherals.pins.gpio22, led_channel_config)
			.expect("Failed to create LED driver");
	led_driver
		.set_pixels_slice(&[
			RGB8::new(255, 0, 0),
			RGB8::new(0, 255, 0),
			RGB8::new(0, 0, 255)
		])
		.unwrap();

	// Create the SPI interfaces
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

	// Make the UART driver
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

	// Make WiFi driver
	let wifi = AsyncWifi::wrap(
		EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs.clone()))
			.unwrap_or_else(|err| panic!("Failed to create inner WiFi driver ({err})")),
		sys_loop,
		timer_service
	)
	.unwrap_or_else(|err| panic!("Failed to create WiFi driver ({err})"));

	let wifi_nvs_partition = EspNvs::new(nvs, "wifi", true).expect("Failed to get NVS namespace");
	let wifi_is_connected = Rc::new(Cell::new(false));

	// Create the HTTP config
	let http_config = HTTPConfiguration {
		crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
		..Default::default()
	};

	// Create a new async embassy task for WiFi.
	spawner.spawn(
		run_wifi(
			wifi,
			uart_driver,
			wifi_nvs_partition,
			wifi_is_connected.clone()
		)
		.unwrap_or_else(|err| panic!("Failed to obtain WiFi task token ({err})"))
	);

	// Wait for WiFi to be connected. todo make loading screen
	while !wifi_is_connected.get() {
		Timer::after_millis(100).await;
	}

	// Create a new async embassy task for the LCD.
	spawner.spawn(
		run_lcd(
			http_config,
			spi3,
			peripherals.pins.gpio5.degrade_output(),
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

	// The PCB has the backlight pin on gpio 25, so I just drive it high for now.
	let mut lcd_led = PinDriver::output(peripherals.pins.gpio25).unwrap();
	lcd_led
		.set_high()
		.expect("Failed to drive LCD backlight high");

	info!("Completed setup");

	loop {
		Timer::after_millis(5).await;
	}
}
