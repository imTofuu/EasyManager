use std::borrow::Borrow;
use std::cell::Cell;
use std::ffi::{CStr, CString, c_int, c_uchar};
use std::ops::Range;
use std::rc::Rc;
use std::str::FromStr;

use anyhow::Context;
use easy_manager_core::get_core_version;
use easy_manager_core::packets::{CLIENT_VERSION_HN, ErrorPacket, Packet};
use embedded_svc::http::Method;
use embedded_svc::http::client::Client;
use embedded_svc::io::Read;
use embedded_svc::wifi::{AuthMethod, ClientConfiguration};
use esp_idf_hal::gpio::OutputPin;
use esp_idf_hal::io::asynch::Write;
use esp_idf_hal::spi::{SpiDeviceDriver, SpiDriver, SpiError};
use esp_idf_hal::sys::{
	esp_eap_client_set_identity,
	esp_eap_client_set_password,
	esp_eap_client_set_username,
	esp_wifi_sta_enterprise_disable,
	esp_wifi_sta_enterprise_enable
};
use esp_idf_hal::uart::{AsyncUartDriver, UartDriver};
use esp_idf_hal::units::Hertz;
use esp_idf_svc::http::client::EspHttpConnection;
use esp_idf_svc::nvs::{EspNvs, NvsDefault};
use esp_idf_svc::wifi::{AsyncWifi, Configuration as WifiConfiguration, EspWifi};
use log::{debug, error, info, warn};
use mfrc522::comm::blocking::spi::{DummyDelay, SpiInterface};
use mfrc522::{Error, Initialized, Mfrc522, MifareKey, Uid};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::lcd::InteractableDisplay;
use crate::uart_read_line;

pub type HttpWork =
	dyn FnOnce(&mut Client<EspHttpConnection>) -> Result<Vec<u8>, ErrorPacket> + Send;
pub type HttpPromiseClosure = dyn FnOnce(Result<Vec<u8>, ErrorPacket>, &mut InteractableDisplay);

pub struct HttpPromise {
	pub work: Box<HttpWork>,
	pub done: Box<HttpPromiseClosure>
}

impl HttpPromise {
	pub fn new<B: Serialize + Send + 'static, R: Serialize + DeserializeOwned + Send + 'static>(
		method: Method,
		uri: String,
		body: Option<B>,
		mut headers: Vec<(String, String)>,
		promise_closure: impl FnOnce(Packet<R>, &mut InteractableDisplay) + 'static
	) -> Self {
		Self {
			work: Box::new(
				move |http: &mut Client<EspHttpConnection>| -> Result<Vec<u8>, ErrorPacket> {
					headers.extend_from_slice(&[
						(CLIENT_VERSION_HN.into(), get_core_version().into()),
						("Content-Type".into(), "application/json".into())
					]);
					let hdrs: Vec<_> = headers
						.iter()
						.map(|(k, v)| (k.as_str(), v.as_str()))
						.collect();
					let mut req = match http.request(method, uri.as_str(), hdrs.as_slice()) {
						Ok(req) => req,
						Err(err) => {
							return Err(ErrorPacket {
								message: format!("Error creating request: {err}")
							});
						}
					};

					if let Some(body) = body {
						let mut serializer = serde_json::Serializer::new(Vec::new());
						if let Err(err) = body.serialize(&mut serializer) {
							return Err(ErrorPacket {
								message: format!("Failed to serialize request body: {err}")
							});
						}
						let body_json = serializer.into_inner();
						if let Err(err) = req.connection().write_all(body_json.as_slice()) {
							return Err(ErrorPacket {
								message: format!("Failed to write request body: {err}")
							});
						}
					}

					let mut res = match req.submit() {
						Ok(res) => res,
						Err(err) => {
							return Err(ErrorPacket {
								message: format!("Error making request: {err}")
							});
						}
					};

					let buffer_size = match res.header("Content-Length") {
						Some(buffer_size) => {
							match usize::from_str(buffer_size) {
								Ok(buffer_size) => buffer_size,
								Err(err) => {
									return Err(ErrorPacket {
										message: format!(
											"Error parsing Content-Length header: {err}"
										)
									});
								}
							}
						}
						None => {
							return Err(ErrorPacket {
								message: "Missing header: Content-Length".to_string()
							});
						}
					};

					let mut buf = vec![0u8; buffer_size];
					if let Err(err) = res.read_exact(buf.as_mut_slice()) {
						return Err(ErrorPacket {
							message: format!("Error reading response body: {err}")
						});
					}

					Ok(buf)
				}
			),
			done: Box::new(|data_result, display| {
				let packet = match data_result {
					Ok(data) => {
						match serde_json::from_slice(data.as_slice()) {
							Ok(packet) => Packet::Ok(packet),
							Err(err1) => {
								match serde_json::from_slice(data.as_slice()) {
									Ok(packet) => Packet::Error(packet),
									Err(err2) => {
										Packet::Error(ErrorPacket {
											message: format!(
												"Failed to parse error packet: {err2}\nafter \
												 failing to parse normal packet: {err1}"
											)
										})
									}
								}
							}
						}
					}
					Err(err) => Packet::Error(err)
				};
				promise_closure(packet, display);
			})
		}
	}
}

type RfidInner<'s, SPI, T> = Mfrc522<SpiInterface<SpiDeviceDriver<'s, SPI>, DummyDelay>, T>;

pub struct RfidReader<'s, SPI: Borrow<SpiDriver<'s>> + 's, K: Fn(&Uid, u8) -> MifareKey> {
	inner:  RfidInner<'s, SPI, Initialized>,
	key_cb: K
}

impl<'s, SPI: Borrow<SpiDriver<'s>> + 's, K: Fn(&Uid, u8) -> MifareKey> RfidReader<'s, SPI, K> {
	pub fn new(spi: SPI, cs: Option<impl OutputPin + 's>, key_cb: K) -> anyhow::Result<Self> {
		let mut rfid_reader_spi_config = esp_idf_hal::spi::config::Config::default();
		rfid_reader_spi_config.baudrate = Hertz(1_000_000);
		let rfid_reader_device = SpiDeviceDriver::new(spi, cs, &rfid_reader_spi_config)
			.context("failed to create RFID reader SPI device")?;
		let rfid_interface = SpiInterface::new(rfid_reader_device);
		let inner = Mfrc522::new(rfid_interface)
			.init()
			.context("failed to initialise RFID reader")?;

		Ok(Self { inner, key_cb })
	}

	pub fn reqa_collect_data(
		&mut self,
		blocks: Range<u8>
	) -> Result<Option<Vec<u8>>, Error<SpiError>> {
		let mut out = Vec::new();
		let atqa = match self.inner.reqa() {
			Ok(atqa) => atqa,
			Err(Error::Timeout) => return Ok(None),
			Err(err) => return Err(err)
		};
		let uid = self.inner.select(&atqa)?;
		for block in blocks {
			self.inner
				.mf_authenticate(&uid, block, &self.key_cb.call((&uid, block)))?;
			out.push(self.inner.mf_read(block)?);
		}
		self.inner.hlta()?;
		self.inner.stop_crypto1()?;
		Ok(Some(out.concat()))
	}

	pub fn reqa_write_data(
		&mut self,
		data: impl IntoIterator<Item = (u8, [u8; 16])>
	) -> Result<bool, Error<SpiError>> {
		let atqa = match self.inner.reqa() {
			Ok(atqa) => atqa,
			Err(Error::Timeout) => return Ok(false),
			Err(err) => return Err(err)
		};
		let uid = self.inner.select(&atqa)?;
		for (block, data) in data {
			self.inner
				.mf_authenticate(&uid, block, &self.key_cb.call((&uid, block)))?;
			while self.inner.mf_read(block)? != data {
				self.inner.mf_write(block, data)?;
			}
		}
		self.inner.hlta()?;
		self.inner.stop_crypto1()?;
		Ok(true)
	}
}

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
