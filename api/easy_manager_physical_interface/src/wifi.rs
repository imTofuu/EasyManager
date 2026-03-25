use std::cell::Cell;
use std::ffi::{CStr, CString, c_int, c_uchar};
use std::rc::Rc;
use std::str::FromStr;

use anyhow::Context;
use embedded_svc::wifi::{AuthMethod, ClientConfiguration};
use esp_idf_hal::io::asynch::Write;
use esp_idf_hal::sys::{
	esp_eap_client_set_identity,
	esp_eap_client_set_password,
	esp_eap_client_set_username,
	esp_wifi_sta_enterprise_disable,
	esp_wifi_sta_enterprise_enable
};
use esp_idf_hal::uart::{AsyncUartDriver, UartDriver};
use esp_idf_svc::nvs::{EspNvs, NvsDefault};
use esp_idf_svc::wifi::{AsyncWifi, Configuration as WifiConfiguration, EspWifi};
use log::{debug, error, info, warn};

use crate::uart_read_line;

async fn create_new_wifi_config(
	uart: &mut AsyncUartDriver<'static, UartDriver<'static>>,
	nvs: &mut EspNvs<NvsDefault>
) -> Result<ClientConfiguration, anyhow::Error> {
	let mut wifi_config = ClientConfiguration::default();

	loop {
		uart.write_all(c"\nEnter auth method: ".to_bytes()).await?;

		match uart_read_line(uart, true).await.as_bytes() {
			b"WPA" => wifi_config.auth_method = AuthMethod::WPA2Personal,
			b"EAP" => wifi_config.auth_method = AuthMethod::WPA2Enterprise,
			_ => {
				error!("Unknown auth method");
				continue;
			}
		}
		break;
	}

	loop {
		uart.write_all(c"\nEnter SSID: ".to_bytes()).await?;

		match uart_read_line(uart, true).await.to_str() {
			Ok(ok) => {
				match ok.try_into() {
					Ok(ok) => {
						wifi_config.ssid = ok;
						break;
					}
					Err(_) => error!("SSID is too long")
				}
			}
			Err(_) => error!("SSID contains invalid characters")
		}
	}

	if let AuthMethod::WPA2Enterprise = wifi_config.auth_method {
		uart.write_all(c"\nEnter identity: ".to_bytes()).await?;
		let identity = uart_read_line(uart, true).await;

		uart.write_all(c"\nEnter username: ".to_bytes()).await?;
		let username = uart_read_line(uart, true).await;

		nvs.set_str(
			"identity",
			identity
				.to_str()
				.context("Converting identity to rust str")?
		)?;
		nvs.set_str(
			"username",
			username
				.to_str()
				.context("Converting username to rust str")?
		)?;
	}

	loop {
		uart.write_all(c"\nEnter password: ".to_bytes()).await?;

		match uart_read_line(uart, false).await.to_str() {
			Ok(input) => {
				match input.try_into() {
					Ok(s32_password) => {
						wifi_config.password = s32_password;
						break;
					}
					Err(_) => error!("Password is too long")
				}
			}
			Err(_) => error!("Password contains invalid characters")
		}
	}

	Ok(wifi_config)
}

async fn get_current_wifi_config(
	wifi: &AsyncWifi<EspWifi<'static>>
) -> Option<ClientConfiguration> {
	match wifi.get_configuration() {
		Ok(config) => {
			match config {
				WifiConfiguration::Client(config) => Some(config),
				_ => None
			}
		}
		Err(err) => {
			error!("Failed to get previous WiFi config ({err})");
			None
		}
	}
}

// Ownership of Wi-Fi is shared with the main thread. If main panics then this
// task will end, so the mutex will never be poisoned in this task, and
// therefore unwraps are safe to use on them.
#[allow(clippy::unwrap_used)]
#[embassy_executor::task]
pub async fn run_wifi(
	mut wifi: AsyncWifi<EspWifi<'static>>,
	mut uart: AsyncUartDriver<'static, UartDriver<'static>>,
	mut nvs: EspNvs<NvsDefault>,
	is_connected: Rc<Cell<bool>>
) -> ! {
	// Setup Wi-Fi until success
	loop {
		match get_current_wifi_config(&wifi).await {
			Some(_) => debug!("Previous WiFi config found"),
			None => {
				warn!("Previous WiFi config is missing or invalid");
				match create_new_wifi_config(&mut uart, &mut nvs).await {
					Ok(config) => {
						if let Err(err) = wifi.set_configuration(&WifiConfiguration::Client(config))
						{
							error!("Failed to apply config to WiFi driver ({err})");
						}
					}
					Err(err) => {
						error!("Failed to set new WiFi config ({err}); retrying");
						continue;
					}
				};
			}
		}

		if let Err(err) = wifi.start().await {
			error!("Failed to start WiFi driver ({err}); retrying setup");
			continue;
		}

		break;
	}

	info!("WiFi started");

	loop {
		is_connected.set(false);
		// Get existing config or create a new one
		let current_config = match get_current_wifi_config(&wifi).await {
			Some(config) => {
				debug!("Previous WiFi config found");
				config
			}
			None => {
				warn!("Previous WiFi config is missing or invalid");
				match create_new_wifi_config(&mut uart, &mut nvs).await {
					Ok(config) => {
						if let Err(err) = wifi.set_configuration(&WifiConfiguration::Client(config))
						{
							error!("Failed to apply config to WiFi driver ({err})");
						}
						continue;
					}
					Err(err) => {
						error!("Failed to set new WiFi config ({err}); retrying");
						if let Err(err) = wifi.set_configuration(&WifiConfiguration::None) {
							error!("Failed to reset WiFi config ({err})");
						}
						continue;
					}
				}
			}
		};

		match current_config.auth_method {
			AuthMethod::WPA2Personal => unsafe {
				esp_wifi_sta_enterprise_disable();
			},
			AuthMethod::WPA2Enterprise => unsafe {
				let mut identity = [0u8; 253];
				if let Err(err) = nvs.get_str("identity", &mut identity) {
					error!("Failed to get identity from NVS ({err})");
					if let Err(err) = wifi.set_configuration(&WifiConfiguration::None) {
						error!("Failed to reset WiFi config ({err})");
					}
					continue;
				}

				let c_identity = match CStr::from_bytes_until_nul(&identity) {
					Ok(result) => result,
					Err(err) => {
						error!("Failed to parse identity as c-string ({err})");
						if let Err(err) = wifi.set_configuration(&WifiConfiguration::None) {
							error!("Failed to reset WiFi config ({err})");
						}
						continue;
					}
				};

				esp_eap_client_set_identity(
					c_identity.as_ptr() as *const c_uchar,
					c_identity.count_bytes() as c_int
				);

				let mut username = [0u8; 253];
				if let Err(err) = nvs.get_str("username", &mut username) {
					error!("Failed to get username from NVS ({err})");
					if let Err(err) = wifi.set_configuration(&WifiConfiguration::None) {
						error!("Failed to reset WiFi config ({err})");
					}
					continue;
				}

				let c_username = match CStr::from_bytes_until_nul(&username) {
					Ok(result) => result,
					Err(err) => {
						error!("Failed to parse username as c-string ({err})");
						if let Err(err) = wifi.set_configuration(&WifiConfiguration::None) {
							error!("Failed to reset WiFi config ({err})");
						}
						continue;
					}
				};

				esp_eap_client_set_username(
					c_username.as_ptr() as *const c_uchar,
					c_username.count_bytes() as c_int
				);

				let c_password = match CString::from_str(current_config.password.as_str()) {
					Ok(result) => result,
					Err(err) => {
						error!("Failed to parse password as c-string ({err})");
						if let Err(err) = wifi.set_configuration(&WifiConfiguration::None) {
							error!("Failed to reset WiFi config ({err})");
						}
						continue;
					}
				};

				esp_eap_client_set_password(
					c_password.as_ptr() as *const c_uchar,
					c_password.count_bytes() as c_int
				);

				esp_wifi_sta_enterprise_enable();
			},
			_ => {
				error!("Invalid auth method");
				if let Err(err) = wifi.set_configuration(&WifiConfiguration::None) {
					error!("Failed to reset WiFi config ({err})");
				}
				continue;
			}
		}

		match wifi.connect().await {
			Ok(()) => {
				info!("WiFi connection succeeded");

				if let Err(err) = wifi.wait_netif_up().await {
					error!("Failed to wait for WiFi interface to go up ({err}); reconnecting");
					continue;
				}

				is_connected.set(true);

				debug!(
					"Connection information: {:?}",
					wifi.wifi().sta_netif().get_ip_info()
				);

				if let Err(err) = wifi.wifi_wait(|wifi| wifi.is_connected(), None).await {
					error!("Failed to wait for disconnection ({err}); reconnecting");
					continue;
				}
			}
			Err(err) => {
				error!("Failed WiFi connection: ({err}); resetting configuration");
				if let Err(err) = wifi.set_configuration(&WifiConfiguration::None) {
					error!("Failed to reset WiFi config ({err})");
				}
				continue;
			}
		};
	}
}
