#![feature(read_array)]
#![feature(int_roundings)]
#![feature(iter_array_chunks)]
#![feature(fn_traits)]

pub mod lcd;
pub mod rfid;
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
use esp_idf_hal::pcnt::PcntUnitDriver;
use esp_idf_hal::pcnt::config::{ChannelConfig, ChannelEdgeAction, ChannelLevelAction, UnitConfig};
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
use crate::rfid::RfidReader;
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

	let spi2 = SpiDriver::new(
		peripherals.spi2,
		peripherals.pins.gpio14,
		peripherals.pins.gpio13,
		Some(peripherals.pins.gpio12),
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

	// Drop these so they aren't accidentally used after giving them to the lcd
	peripherals.pins.gpio15;
	peripherals.pins.gpio26;
	peripherals.pins.gpio27;

	let display =
		setup_lcd(&spi2, 15, 26, 27).unwrap_or_else(|err| panic!("Failed to setup LCD ({err})"));

	debug!("Created display");

	let pcnt_config = UnitConfig {
		low_limit: i16::MIN as i32,
		high_limit: i16::MAX as i32,
		intr_priority: 0,
		accum_count: false,
		..Default::default()
	};
	let mut pulse_counter_driver = PcntUnitDriver::new(&pcnt_config)
		.unwrap_or_else(|err| panic!("Failed to create pulse counter driver ({err})"));
	pulse_counter_driver
		.add_channel(
			Some(peripherals.pins.gpio32),
			Some(peripherals.pins.gpio33),
			&ChannelConfig::default()
		)
		.unwrap_or_else(|err| panic!("Failed to add channel to pulse counter ({err})"))
		.set_edge_action(ChannelEdgeAction::Decrease, ChannelEdgeAction::Increase)
		.unwrap_or_else(|err| panic!("Failed to set edge action of pulse counter channel ({err})"))
		.set_level_action(ChannelLevelAction::Keep, ChannelLevelAction::Inverse)
		.unwrap_or_else(|err| {
			panic!("Failed to set level action of pulse counter channel ({err})")
		});

	pulse_counter_driver
		.enable()
		.unwrap_or_else(|err| panic!("Failed to enable pulse counter ({err})"));
	pulse_counter_driver
		.start()
		.unwrap_or_else(|err| panic!("Failed to start pulse counter ({err})"));

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
		run_lcd(display, pulse_counter_driver)
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

	let http_config = HTTPConfiguration {
		crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
		..Default::default()
	};

	let mut http_client =
		Client::wrap(EspHttpConnection::new(&http_config).expect("Failed to create HTTP client"));

	debug!("Created HTTP client");

	info!("Completed setup");

	let mut rfid_device = RfidReader::new(&spi3, Some(peripherals.pins.gpio5), |uid, _| {
		info!("uid: {:?}", uid.as_bytes());
		[0xff; 6]
	})
	.unwrap_or_else(|err| panic!("Failed to create RFID device ({err})"));

	loop {
		Timer::after_millis(50).await;

		match rfid_device.reqa_collect_data(1..2) {
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
