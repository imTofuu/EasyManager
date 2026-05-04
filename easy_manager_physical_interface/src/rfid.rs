use std::borrow::Borrow;
use std::ops::Range;

use anyhow::Context;
use esp_idf_hal::gpio::OutputPin;
use esp_idf_hal::spi::{SpiDeviceDriver, SpiDriver, SpiError};
use esp_idf_hal::units::Hertz;
use mfrc522::comm::blocking::spi::{DummyDelay, SpiInterface};
use mfrc522::{Error, Initialized, Mfrc522, MifareKey, State, Uid};

type RfidInner<'d, SPI: Borrow<SpiDriver<'d>> + 'd, T: State> =
	Mfrc522<SpiInterface<SpiDeviceDriver<'d, SPI>, DummyDelay>, T>;

pub struct RfidReader<'d, SPI: Borrow<SpiDriver<'d>> + 'd, K: Fn(&Uid, u8) -> MifareKey> {
	inner:  RfidInner<'d, SPI, Initialized>,
	key_cb: K
}

impl<'d, SPI: Borrow<SpiDriver<'d>> + 'd, K: Fn(&Uid, u8) -> MifareKey> RfidReader<'d, SPI, K> {
	pub fn new(spi: SPI, cs: Option<impl OutputPin + 'd>, key_cb: K) -> anyhow::Result<Self> {
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
