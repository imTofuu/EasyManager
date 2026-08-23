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
use uuid::Uuid;

use crate::lcd::InteractableDisplay;
use crate::uart_read_line;

pub type HttpWorkResult = Result<(u16, Option<Option<Uuid>>, Vec<u8>), ErrorPacket>;
pub type HttpWork =
	dyn FnOnce(&mut Client<EspHttpConnection>, &Option<Uuid>) -> HttpWorkResult + Send;
pub type HttpPromiseClosure =
	dyn FnOnce(Result<(u16, Vec<u8>), ErrorPacket>, &mut InteractableDisplay);

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
				move |http: &mut Client<EspHttpConnection>,
				      session_id: &Option<Uuid>|
				      -> HttpWorkResult {
					
					// Serialise body
					let body_json = match body {
						Some(body) => {
							let mut serializer = serde_json::Serializer::new(Vec::new());
							if let Err(err) = body.serialize(&mut serializer) {
								return Err(ErrorPacket {
									message: format!("Failed to serialize request body: {err}")
								});
							}
							serializer.into_inner()
						}
						None => Vec::new()
					};

					// Add default headers to user headers
					headers.extend_from_slice(&[
						(CLIENT_VERSION_HN.into(), get_core_version().into()),
						(
							"Cookie".into(),
							format!(
								"session={}",
								session_id
									.map(|session_id| session_id.to_string())
									.unwrap_or("".to_string())
							)
						),
						("Content-Type".into(), "application/json".into()),
						("Content-Length".into(), body_json.len().to_string())
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

					if let Err(err) = req.connection().write_all(body_json.as_slice()) {
						return Err(ErrorPacket {
							message: format!("Failed to write request body: {err}")
						});
					}

					// Make request
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
						None => 0
					};

					// Read response body
					let mut buf = vec![0u8; buffer_size];
					if let Err(err) = res.read_exact(buf.as_mut_slice()) {
						return Err(ErrorPacket {
							message: format!("Error reading response body: {err}")
						});
					}

					// Parse new session id if it was returned
					let session_id = res
						.header("set-cookie")
						.and_then(|str| {
							let session_set_start = str.find("session")?;
							Some(&str[session_set_start..])
						})
						.map(|str| {
							let session_id_start = str.find('=').ok_or("Malformed session id")? + 1;
							let str = &str[session_id_start..];
							let session_id_end = str.find(';').ok_or("Malformed session id")?;
							Ok(if session_id_end == 0 {
								None
							} else {
								Some(
									Uuid::try_parse(&str[..session_id_end])
										.map_err(|_| "Failed to parse uuid")?
								)
							})
						})
						.transpose()
						.map_err(|msg: &str| {
							ErrorPacket {
								message: msg.to_string()
							}
						})?
						.or_else(|| Some(session_id.clone()));

					Ok((res.status(), session_id, buf))
				}
			),
			done: Box::new(|data_result, display| {
				let packet = match data_result {
					Ok((code, data)) => {
						// Serialise packet
						if data.len() > 0 {
							match serde_json::from_slice(data.as_slice()) {
								Ok(packet) => packet,
								Err(err) => {
									Packet::Error(code, ErrorPacket {
										message: format!("Failed to parse error packet: {err}")
									})
								}
							}
						} else {
							Packet::None(200)
						}
					}
					Err(err) => Packet::Error(0, err)
				};
				promise_closure(packet, display);
			})
		}
	}
}

pub type RfidReadResult = Result<Option<Vec<[u8; 16]>>, Error<SpiError>>;
pub type RfidReadPromise =
	Box<dyn FnOnce(Result<Vec<[u8; 16]>, Error<SpiError>>, &mut InteractableDisplay) + 'static>;

pub type RfidWriteResult = Result<bool, Error<SpiError>>;
pub type RfidWritePromise =
	Box<dyn FnOnce(Result<(), Error<SpiError>>, &mut InteractableDisplay) + 'static>;

pub enum RfidPromise {
	Read(
		Box<
			dyn for<'a> Fn(
				&mut RfidReader<'a, SpiDriver<'a>, Box<dyn Fn(&Uid, u8) -> MifareKey>>
			) -> RfidReadResult
		>,
		RfidReadPromise
	),
	Write(
		Box<
			dyn for<'a> Fn(
				&mut RfidReader<'a, SpiDriver<'a>, Box<dyn Fn(&Uid, u8) -> MifareKey>>
			) -> RfidWriteResult
		>,
		RfidWritePromise
	)
}

impl RfidPromise {
	pub fn read<'a>(blocks: Range<u8>, promise_closure: RfidReadPromise) -> Self {
		Self::Read(
			Box::new(move |rfid| rfid.reqa_collect_data(blocks.clone())),
			promise_closure
		)
	}

	pub fn write(
		data: Vec<(u8, [u8; 16])>,
		default: bool,
		promise_closure: RfidWritePromise
	) -> Self {
		Self::Write(
			Box::new(move |rfid| rfid.reqa_write_data(data.clone(), default)),
			promise_closure
		)
	}
}

type RfidInner<'s, SPI, T> = Mfrc522<SpiInterface<SpiDeviceDriver<'s, SPI>, DummyDelay>, T>;

pub struct RfidReader<'s, SPI: Borrow<SpiDriver<'s>> + 's, K: Fn(&Uid, u8) -> MifareKey> {
	inner:  RfidInner<'s, SPI, Initialized>,
	key_cb: K
}

impl<'s, SPI: Borrow<SpiDriver<'s>> + 's, K: Fn(&Uid, u8) -> MifareKey> RfidReader<'s, SPI, K> {
	pub fn new(spi: SPI, cs: Option<impl OutputPin + 's>, key_cb: K) -> anyhow::Result<Self> {
		let rfid_reader_spi_config = esp_idf_hal::spi::config::Config {
			baudrate: Hertz(1_000_000),
			..Default::default()
		};
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
	) -> Result<Option<Vec<[u8; 16]>>, Error<SpiError>> {
		let mut out = Vec::new();
		let atqa = match self.inner.reqa() {
			Ok(atqa) => atqa,
			Err(Error::Timeout) => return Ok(None),
			Err(err) => return Err(err)
		};
		let uid = self.inner.select(&atqa)?;
		for block in blocks {
			self.inner
				.mf_authenticate(&uid, block, &self.get_auth_for_block(&uid, block))?;
			out.push(self.inner.mf_read(block)?);
		}
		self.inner.hlta()?;
		self.inner.stop_crypto1()?;
		Ok(Some(out))
	}

	pub fn reqa_write_data(
		&mut self,
		data: impl IntoIterator<Item = (u8, [u8; 16])> + Clone,
		default: bool
	) -> Result<bool, Error<SpiError>> {
		let atqa = match self.inner.reqa() {
			Ok(atqa) => atqa,
			Err(Error::Timeout) => return Ok(false),
			Err(err) => return Err(err)
		};
		let uid = self.inner.select(&atqa)?;

		let mut written_key_sectors = Vec::new();

		for (block, data) in data {
			if block % 4 == 3 {
				panic!("trying to write to trailer block")
			}

			if default {
				let sector = block / 4;
				if !written_key_sectors.contains(&sector) {
					let trailer_block = block + (3 - (block % 4));

					self.inner
						.mf_authenticate(&uid, trailer_block, &[0xff; 6])?;

					let mut new_key = self.inner.mf_read(trailer_block)?;
					new_key[..6].copy_from_slice(&self.get_auth_for_block(&uid, block));
					self.inner.mf_write(trailer_block, new_key)?;
					written_key_sectors.push(sector);
				}
			}

			self.inner
				.mf_authenticate(&uid, block, &self.get_auth_for_block(&uid, block))?;

			self.inner.mf_write(block, data)?;
		}
		self.inner.hlta()?;
		self.inner.stop_crypto1()?;
		Ok(true)
	}

	fn get_auth_for_block(&self, uid: &Uid, block: u8) -> MifareKey {
		self.key_cb.call((uid, block / 4))
	}
}

// Uses UART to get new WiFi credentials
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

// Ownership of WiFi is shared with the main thread. If main panics then this
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
	// Get WiFi config or create a new one until one exists
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

	// Connect to WiFi or change config forever
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

				// Wait until disconnect
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
