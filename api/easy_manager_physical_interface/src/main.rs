use std::ffi::{CString, c_int, c_uchar};
use std::panic;
use std::panic::PanicHookInfo;

use embassy_executor::Spawner;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::gpio::{Gpio0, Gpio1};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::uart::config::Config;
use esp_idf_svc::hal::uart::{AsyncUartDriver, UartDriver};
use esp_idf_svc::hal::units::Hertz;
use esp_idf_svc::io::asynch::{Read, Write};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys::{EspError, esp_eap_client_set_identity, esp_eap_client_set_username};
use esp_idf_svc::timer::EspTaskTimerService;
use esp_idf_svc::wifi::{AsyncWifi, AuthMethod, ClientConfiguration, Configuration, EspWifi};
use log::{error, info};

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

async fn create_new_wifi_config(
	uart: &mut AsyncUartDriver<'static, UartDriver<'static>>
) -> Configuration {
	let auth_method;

	loop {
		uart.write_all(c"\nEnter auth method: ".to_bytes())
			.await
			.unwrap_or_else(|err| panic!("Failed to write to uart ({err})"));

		match uart_read_line(uart, true).await.as_bytes() {
			b"WPA" => auth_method = AuthMethod::WPA2Personal,
			b"EAP" => auth_method = AuthMethod::WPA2Enterprise,
			_ => {
				error!("Unknown auth method");
				continue;
			}
		}
		break;
	}

	let ssid;

	loop {
		uart.write_all(c"\nEnter SSID: ".to_bytes())
			.await
			.unwrap_or_else(|err| panic!("Failed to write to uart ({err})"));

		match uart_read_line(uart, true).await.to_str() {
			Ok(ok) => {
				match ok.try_into() {
					Ok(ok) => {
						ssid = ok;
						break;
					}
					Err(_) => error!("SSID is too long")
				}
			}
			Err(_) => error!("SSID contains invalid characters")
		}
	}

	if let AuthMethod::WPA2Enterprise = auth_method {
		uart.write_all(c"\nEnter identity: ".to_bytes())
			.await
			.unwrap_or_else(|err| panic!("Failed to write to uart ({err})"));
		let identity = uart_read_line(uart, true).await;

		uart.write_all(c"\nEnter username: ".to_bytes())
			.await
			.unwrap_or_else(|err| panic!("Failed to write to uart ({err})"));
		let username = uart_read_line(uart, true).await;

		unsafe {
			// SAFETY: identity is guaranteed to be a valid c_str before this call
			if let Err(err) = EspError::convert(esp_eap_client_set_identity(
				identity.as_ptr() as *const c_uchar,
				identity.count_bytes() as c_int
			)) {
				panic!("Failed to set EAP identity ({err})");
			}

			// SAFETY: username is guaranteed to be a valid c_str before this call
			if let Err(err) = EspError::convert(esp_eap_client_set_username(
				username.as_ptr() as *const c_uchar,
				username.count_bytes() as c_int
			)) {
				panic!("Failed to set EAP username ({err})");
			}
		}
	}

	let password;

	loop {
		uart.write_all(c"\nEnter password: ".to_bytes())
			.await
			.unwrap_or_else(|err| panic!("Failed to write to uart ({err})"));

		match uart_read_line(uart, false).await.to_str() {
			Ok(input) => {
				match input.try_into() {
					Ok(s32_password) => {
						password = s32_password;
						break;
					}
					Err(_) => error!("Password is too long")
				}
			}
			Err(_) => error!("Password contains invalid characters")
		}
	}

	let wifi_config = Configuration::Client(ClientConfiguration {
		ssid,
		password,
		auth_method,
		..Default::default()
	});

	wifi_config
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
	esp_idf_svc::sys::link_patches();

	esp_idf_svc::log::init_from_esp_idf();

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

	let uart_tx_pin = peripherals.pins.gpio1;
	let uart_rx_pin = peripherals.pins.gpio3;

	let mut uart_config = Config::new();
	uart_config.baudrate = Hertz(115200);

	let mut uart_driver = AsyncUartDriver::new(
		peripherals.uart0,
		uart_tx_pin,
		uart_rx_pin,
		None::<Gpio0>,
		None::<Gpio1>,
		&uart_config
	)
	.unwrap_or_else(|err| panic!("Failed to create UART driver ({err})"));

	info!("Gotten ESP services");

	let mut wifi = AsyncWifi::wrap(
		EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs))
			.unwrap_or_else(|err| panic!("Failed to create inner WiFi driver ({err})")),
		sys_loop,
		timer_service
	)
	.unwrap_or_else(|err| panic!("Failed to create WiFi driver ({err})"));
	
	match wifi.get_configuration() {
		Ok(config) => {
			if let Configuration::None = config {
				info!("WiFi config was previously reset.");
				wifi.set_configuration(&create_new_wifi_config(&mut uart_driver).await)
					.unwrap_or_else(|err| panic!("Failed set set WiFi config ({err})"));
			}
		},
		Err(_) => {
			error!("Error getting WiFi config");
			wifi.set_configuration(&create_new_wifi_config(&mut uart_driver).await)
				.unwrap_or_else(|err| panic!("Failed set set WiFi config ({err})"));
		}
	}

	wifi.start()
		.await
		.unwrap_or_else(|err| panic!("Failed to start WiFi ({err})"));
	info!("WiFi started");

	wifi.connect().await.unwrap_or_else(|err| {
		error!("Failed to connect to WiFi; resetting config");
		wifi.set_configuration(&Configuration::None)
			.unwrap_or_else(|err| panic!("Failed to reset WiFi config ({err})"));
		panic!("Failed to connect to WiFi ({err})")
	});
	info!("WiFi connected");

	wifi.wait_netif_up()
		.await
		.unwrap_or_else(|err| panic!("Failed to wait for WiFi interface to go up ({err})"));

	info!(
		"WiFi DHCP info: {:?}",
		wifi.wifi()
			.sta_netif()
			.get_ip_info()
			.unwrap_or_else(|err| panic!("Failed to get WiFi interface info ({err})"))
	);
	
	wifi.stop().await.unwrap_or_else(|err| panic!("Failed to stop WiFi driver ({err})"));
}
