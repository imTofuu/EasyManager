use std::ffi::{c_int, c_void};
use std::io::Read;
use std::mem::transmute;
use std::time::Instant;

use anyhow::Context;
use cstr_core::CStr;
use embassy_time::Timer;
use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::{Point, Size};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::primitives::Rectangle;
use embedded_sdmmc::{BlockDevice, File, TimeSource, Timestamp};
use esp_idf_hal::gpio::{OutputPin, Pin, PinDriver, PinId};
use esp_idf_hal::spi::SpiDriver;
use esp_idf_sys::{
	esp,
	esp_lcd_new_panel_io_spi,
	esp_lcd_new_panel_st7789,
	esp_lcd_panel_dev_config_t,
	esp_lcd_panel_disp_on_off,
	esp_lcd_panel_draw_bitmap,
	esp_lcd_panel_handle_t,
	esp_lcd_panel_init,
	esp_lcd_panel_invert_color,
	esp_lcd_panel_io_handle_t,
	esp_lcd_panel_io_spi_config_t,
	esp_lcd_panel_reset,
	esp_lcd_panel_set_gap,
	esp_lcd_spi_bus_handle_t,
	lcd_rgb_data_endian_t_LCD_RGB_DATA_ENDIAN_LITTLE
};
use log::error;
use lvgl::Align::Center;
use lvgl::style::Style;
use lvgl::widgets::Label;
use lvgl::{Color, Display, DrawBuffer, Part, Widget};

pub struct SPIPinDriver<'d, MODE>(PinDriver<'d, MODE>);

impl<'d, MODE> SPIPinDriver<'d, MODE> {
	pub fn new(pin_driver: PinDriver<'d, MODE>) -> Self { Self(pin_driver) }
}

impl<'d, MODE> Pin for SPIPinDriver<'d, MODE> {
	fn pin(&self) -> PinId { self.0.pin() }
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

pub fn setup_lcd(spi: &SpiDriver, cs: c_int, dc: c_int, rst: c_int) -> anyhow::Result<Display> {
	let panel_io_config = esp_lcd_panel_io_spi_config_t {
		dc_gpio_num: dc,
		cs_gpio_num: cs,
		pclk_hz: 10 * 1000 * 1000,
		lcd_cmd_bits: 8,
		lcd_param_bits: 8,
		spi_mode: 0,
		trans_queue_depth: 1,
		..Default::default()
	};

	let panel_dev_config = esp_lcd_panel_dev_config_t {
		reset_gpio_num: rst,
		data_endian: lcd_rgb_data_endian_t_LCD_RGB_DATA_ENDIAN_LITTLE,
		bits_per_pixel: 16,
		..Default::default()
	};

	let mut io_handle = esp_lcd_panel_io_handle_t::default();
	let mut panel_handle = esp_lcd_panel_handle_t::default();

	unsafe {
		esp!(esp_lcd_new_panel_io_spi(
			spi.host() as esp_lcd_spi_bus_handle_t,
			&panel_io_config,
			&mut io_handle
		))
		.context("failed to create LCD spi interface")?;
		esp!(esp_lcd_new_panel_st7789(
			io_handle,
			&panel_dev_config,
			&mut panel_handle
		))
		.context("failed to create LCD interface")?;
		esp_lcd_panel_reset(panel_handle);
		esp_lcd_panel_init(panel_handle);
		esp_lcd_panel_set_gap(panel_handle, 0, 0);
		esp_lcd_panel_invert_color(panel_handle, true);
		esp_lcd_panel_disp_on_off(panel_handle, true);
	}

	let lvgl_buffer = DrawBuffer::<4800usize>::default();
	let lvgl_display = Display::register(lvgl_buffer, 240, 320, move |refresh| {
		esp!(unsafe {
			esp_lcd_panel_draw_bitmap(
				panel_handle,
				refresh.area.x1 as c_int,
				refresh.area.y1 as c_int,
				(refresh.area.x2 + 1) as c_int,
				(refresh.area.y2 + 1) as c_int,
				refresh.colors.as_ptr() as *const c_void
			)
		})
		.unwrap_or_else(|err| error!("Failed to draw bitmap ({err})"));
	})
	.context("failed to create LVGL display")?;

	let mut screen = lvgl_display
		.get_scr_act()
		.context("failed to get active LVGL screen")?;

	let mut screen_style = Style::default();
	screen_style.set_bg_color(Color::from_rgb((0, 255, 0)));
	screen_style.set_text_color(Color::from_rgb((255, 255, 255)));
	screen_style.set_radius(0);
	screen.add_style(Part::Main, &mut screen_style);

	let mut label: Label = Label::create(&mut screen)?;
	label.set_text(CStr::from_bytes_with_nul(b"hello\0").unwrap());
	label.set_height(50);
	label.set_width(50);
	label.set_align(Center, 0, 0);

	Ok(lvgl_display)
}

#[embassy_executor::task]
pub async fn run_lcd() -> ! {
	let mut last = Instant::now();
	loop {
		let now = Instant::now();
		lvgl::task_handler();
		lvgl::tick_inc(now - last);
		last = now;
		Timer::after_millis(50).await;
	}
}

pub fn chunked_draw_from_bitmap_reader<D: DrawTarget<Color = Rgb565>>(
	display: &mut D,
	mut reader: impl Read,
	chunk_height: u32
) -> std::io::Result<()> {
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
			Point::new(
				0,
				(display.bounding_box().size.height - (chunk_height * chunk_index)) as i32
			),
			Size::new(display.bounding_box().size.width, chunk_height)
		)
		.intersection(&display.bounding_box());

		display
			.fill_contiguous(
				&chunk_bounds,
				data_bytes
					.into_iter()
					.take((padded_row_byte_length * chunk_bounds.size.height) as usize)
					.array_chunks::<2>()
					.map(|bytes| unsafe { transmute(((bytes[1] as u16) << 8) | (bytes[0] as u16)) })
					.rev()
			)
			.unwrap_or_else(|_| panic!("Failed to draw image chunk"));
	}

	Ok(())
}
