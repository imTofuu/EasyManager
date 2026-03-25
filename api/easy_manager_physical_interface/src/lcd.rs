use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::{Point, Size};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::primitives::Rectangle;
use embedded_sdmmc::{BlockDevice, File, TimeSource, Timestamp};
use esp_idf_hal::gpio::{OutputPin, Pin, PinDriver, PinId};
use std::io::Read;
use std::mem::transmute;

pub struct SPIPinDriver<'d, MODE> {
	inner: PinDriver<'d, MODE>
}

impl<'d, MODE> SPIPinDriver<'d, MODE> {
	pub fn new(pin_driver: PinDriver<'d, MODE>) -> Self { Self { inner: pin_driver } }
}

impl<'d, MODE> Pin for SPIPinDriver<'d, MODE> {
	fn pin(&self) -> PinId { self.inner.pin() }
}

impl<'d, MODE> OutputPin for SPIPinDriver<'d, MODE> {}

pub struct DummyTimesource();

impl TimeSource for DummyTimesource {
	fn get_timestamp(&self) -> Timestamp { Timestamp::from_calendar(1970, 1, 1, 0, 0, 0).unwrap() }
}

pub struct ReadableFile<
	'a,
	D: BlockDevice,
	T: TimeSource,
	const MAX_DIRS: usize,
	const MAX_FILES: usize,
	const MAX_VOLUMES: usize
>(File<'a, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>);

impl<
	'a,
	D: BlockDevice,
	T: TimeSource,
	const MAX_DIRS: usize,
	const MAX_FILES: usize,
	const MAX_VOLUMES: usize
> ReadableFile<'a, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>
{
	pub fn new(inner: File<'a, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>) -> Self { Self(inner) }
}

impl<
	'a,
	D: BlockDevice,
	T: TimeSource,
	const MAX_DIRS: usize,
	const MAX_FILES: usize,
	const MAX_VOLUMES: usize
> Read for ReadableFile<'a, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>
{
	fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
		self.0.read(buf).map_err(|_| todo!())
	}
}

pub enum ImageDrawError {
	IOError(std::io::Error),
	DrawError()
}

pub fn chunked_draw_from_bitmap_reader<D: DrawTarget<Color = Rgb565>>(
	display: &mut D,
	mut reader: impl Read,
	chunk_height: u32,
) -> std::io::Result<()>
where  {
	let header: [u8; 26] = reader.read_array()?;

	let pixel_data_location = ((header[13] as u32) << 24)
		| ((header[12] as u32) << 16)
		| ((header[11] as u32) << 8)
		| (header[10] as u32);

	let image_width = ((header[21] as i32) << 24)
		| ((header[20] as i32) << 16)
		| ((header[19] as i32) << 8)
		| (header[18] as i32);

	let image_height = ((header[25] as i32) << 24)
		| ((header[24] as i32) << 16)
		| ((header[23] as i32) << 8)
		| (header[22] as i32);
	
	{
		let mut sink = vec![0u8; (pixel_data_location - 26) as usize];
		reader.read_exact(&mut sink)?;
	}
	
	let row_byte_length = 2 * image_width as u32;
	let padded_row_byte_length = row_byte_length + (row_byte_length % 4);
	let num_chunks = (image_height as u32).div_ceil(chunk_height);
	
	for chunk_index in 1..=num_chunks {
		let mut data_bytes: Vec<u8> = vec![0u8; (padded_row_byte_length * chunk_height) as usize];
		reader.read(&mut data_bytes)?;
		
		for i in row_byte_length..padded_row_byte_length {
			data_bytes.remove(i as usize);
		}
		
		let chunk_bounds = Rectangle::new(
			Point::new(0, (display.bounding_box().size.height - (chunk_height * chunk_index)) as i32),
			Size::new(display.bounding_box().size.width, chunk_height)
		).intersection(&display.bounding_box());
		
		display.fill_contiguous(&chunk_bounds, data_bytes.into_iter().take((padded_row_byte_length * chunk_bounds.size.height) as usize).array_chunks::<2>().map(|bytes| {
			unsafe { transmute(((bytes[1] as u16) << 8) | (bytes[0] as u16)) }
		}).rev())
			.unwrap_or_else(|_| panic!("Failed to draw image chunk"));
	}

	Ok(())
}