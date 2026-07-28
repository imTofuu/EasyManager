use std::ffi::{c_int, c_void};
use std::io::Read;
use std::sync::atomic::{AtomicI16, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::Context;
use arrayvec::ArrayVec;
use embassy_futures::select::{Either, Select, select};
use embassy_time::Timer;
use embedded_sdmmc::{BlockDevice, File, TimeSource, Timestamp};
use esp_idf_hal::gpio::{
	AnyInputPin,
	AnyOutputPin,
	Input,
	InputPin,
	InterruptType,
	Level,
	OutputPin,
	Pin,
	PinDriver,
	PinId,
	Pull
};
use esp_idf_hal::spi::SpiDriver;
use esp_idf_sys::{
	esp,
	esp_lcd_new_panel_io_spi,
	esp_lcd_new_panel_st7796,
	esp_lcd_panel_dev_config_t,
	esp_lcd_panel_dev_config_t__bindgen_ty_1,
	esp_lcd_panel_disp_on_off,
	esp_lcd_panel_draw_bitmap,
	esp_lcd_panel_handle_t,
	esp_lcd_panel_init,
	esp_lcd_panel_invert_color,
	esp_lcd_panel_io_handle_t,
	esp_lcd_panel_io_spi_config_t,
	esp_lcd_panel_mirror,
	esp_lcd_panel_reset,
	esp_lcd_panel_set_gap,
	esp_lcd_spi_bus_handle_t,
	gpio_num_t,
	lcd_rgb_data_endian_t_LCD_RGB_DATA_ENDIAN_LITTLE,
	lcd_rgb_element_order_t_LCD_RGB_ELEMENT_ORDER_BGR,
	st7796_lcd_init_cmd_t,
	st7796_vendor_config_t
};
use log::{debug, error, info};
use lvgl::sys::{
	_lv_indev_drv_t,
	lv_group_create,
	lv_group_set_default,
	lv_indev_data_t,
	lv_indev_drv_register,
	lv_indev_set_group,
	lv_indev_state_t,
	lv_indev_state_t_LV_INDEV_STATE_PRESSED,
	lv_indev_state_t_LV_INDEV_STATE_RELEASED,
	lv_indev_t
};
use lvgl::{Display, DrawBuffer};
use mfrc522::{MifareKey, Uid};

use crate::communications::{CommunicationManager, RfidReader};
use crate::graphics::{ModernTheme, Unipage};
use crate::pages::main_page;

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

pub struct InteractableDisplay<'a> {
	display:    Display,
	page_stack: ArrayVec<Box<Unipage<'a>>, 64>,
	indev:      *mut lv_indev_t,

	enc_clk: PinDriver<'static, Input>,
	enc_sw:  PinDriver<'static, Input>
}

struct EncoderState {
	diff:    i16,
	pressed: lv_indev_state_t
}

static mut ENCODER_STATE: EncoderState = EncoderState {
	diff:    0,
	pressed: lv_indev_state_t_LV_INDEV_STATE_RELEASED
};

impl<'a> InteractableDisplay<'a> {
	pub fn new(
		spi: &SpiDriver,
		cs: impl OutputPin,
		dc: impl OutputPin,
		rst: impl OutputPin,
		enc_clk: impl InputPin + 'static,
		enc_dt: impl InputPin + 'static,
		enc_sw: impl InputPin + 'static
	) -> anyhow::Result<Self> {
		let mut clk = PinDriver::input(enc_clk, Pull::Floating)
			.unwrap_or_else(|err| panic!("Failed to make rotary encoder CLK pin driver ({err})"));
		let dt = PinDriver::input(enc_dt, Pull::Floating)
			.unwrap_or_else(|err| panic!("Failed to make rotary encoder DT pin driver ({err})"));
		let mut sw = PinDriver::input(enc_sw, Pull::Floating)
			.unwrap_or_else(|err| panic!("Failed to make rotary encoder SW pin driver ({err})"));

		let sw_id = sw.pin();

		let perma_count = Arc::new(AtomicI16::new(0));
		let isr_perma_count = perma_count.clone();
		unsafe {
			clk.subscribe(move || {
				let level = match dt.get_level() {
					Level::High => -1,
					Level::Low => 1
				};
				ENCODER_STATE.diff += level;
				isr_perma_count.fetch_add(level, Ordering::Relaxed);
			})
			.context("failed to add rotary encoder CLK interrupt")?;

			sw.subscribe(move || {
				let level = esp_idf_sys::gpio_get_level(sw_id as gpio_num_t);
				let state = if level != 1 {
					lv_indev_state_t_LV_INDEV_STATE_PRESSED
				} else {
					lv_indev_state_t_LV_INDEV_STATE_RELEASED
				};
				ENCODER_STATE.pressed = state;
			})
			.context("failed to add rotary encoder SW interrupt")?;
		}

		clk.set_interrupt_type(InterruptType::PosEdge)
			.context("failed to set rotary encoder CLK interrupt type")?;
		clk.enable_interrupt()
			.context("failed to enable rotary encoder CLK interrupt")?;

		sw.set_interrupt_type(InterruptType::AnyEdge)
			.context("failed to set rotary encoder SW interrupt type")?;
		sw.enable_interrupt()
			.context("failed to enable rotary encoder Sw interrupt")?;

		let panel_io_config = esp_lcd_panel_io_spi_config_t {
			dc_gpio_num: dc.pin() as c_int,
			cs_gpio_num: cs.pin() as c_int,
			pclk_hz: 80 * 1000 * 1000,
			lcd_cmd_bits: 8,
			lcd_param_bits: 8,
			spi_mode: 0,
			trans_queue_depth: 1,
			..Default::default()
		};

		static CSCON_UNLOCK_1: [u8; 1] = [0xc3];
		static CSCON_UNLOCK_2: [u8; 1] = [0x96];
		static PGAMCTRL: [u8; 14] = [
			0xf0, 0x09, 0x13, 0x12, 0x12, 0x2b, 0x3c, 0x44, 0x4b, 0x1b, 0x18, 0x17, 0x1d, 0x21
		];
		static NGAMCTRL: [u8; 14] = [
			0xf0, 0x09, 0x13, 0x0c, 0x0d, 0x27, 0x3b, 0x44, 0x4d, 0x0b, 0x17, 0x17, 0x1d, 0x21
		];

		let init_cmds: [st7796_lcd_init_cmd_t; 4] = [
			st7796_lcd_init_cmd_t {
				cmd:        0xf0,
				data:       CSCON_UNLOCK_1.as_ptr() as *const c_void,
				data_bytes: CSCON_UNLOCK_1.len(),
				delay_ms:   0
			},
			st7796_lcd_init_cmd_t {
				cmd:        0xf0,
				data:       CSCON_UNLOCK_2.as_ptr() as *const c_void,
				data_bytes: CSCON_UNLOCK_2.len(),
				delay_ms:   0
			},
			st7796_lcd_init_cmd_t {
				cmd:        0xe0,
				data:       PGAMCTRL.as_ptr() as *const c_void,
				data_bytes: PGAMCTRL.len(),
				delay_ms:   0
			},
			st7796_lcd_init_cmd_t {
				cmd:        0xe1,
				data:       NGAMCTRL.as_ptr() as *const c_void,
				data_bytes: NGAMCTRL.len(),
				delay_ms:   0
			}
		];

		let mut vendor_config = st7796_vendor_config_t {
			init_cmds: init_cmds.as_ptr(),
			init_cmds_size: init_cmds.len() as u16,
			..Default::default()
		};

		let panel_dev_config = esp_lcd_panel_dev_config_t {
			reset_gpio_num: rst.pin() as c_int,
			data_endian: lcd_rgb_data_endian_t_LCD_RGB_DATA_ENDIAN_LITTLE,
			bits_per_pixel: 16,
			vendor_config: &mut vendor_config as *mut st7796_vendor_config_t as *mut c_void,
			__bindgen_anon_1: esp_lcd_panel_dev_config_t__bindgen_ty_1 {
				rgb_ele_order: lcd_rgb_element_order_t_LCD_RGB_ELEMENT_ORDER_BGR
			},
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
			esp!(esp_lcd_new_panel_st7796(
				io_handle,
				&panel_dev_config,
				&mut panel_handle
			))
			.context("failed to create LCD interface")?;
			esp_lcd_panel_reset(panel_handle);
			esp_lcd_panel_init(panel_handle);
			esp_lcd_panel_set_gap(panel_handle, 0, 0);
			esp_lcd_panel_invert_color(panel_handle, false);
			esp_lcd_panel_mirror(panel_handle, false, true);
			esp_lcd_panel_disp_on_off(panel_handle, true);
		}

		debug!("Initialised LCD");

		let draw_buffer = DrawBuffer::<{ 320 * 10 }>::default();
		let lvgl_display = Display::register(draw_buffer, 320, 480, move |refresh| {
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
		.context("failed to create LVGL _display")?;

		let encoder = Box::leak(Box::new(_lv_indev_drv_t::default()));
		unsafe { lvgl::sys::lv_indev_drv_init(encoder) };
		encoder.type_ = lvgl::sys::lv_indev_type_t_LV_INDEV_TYPE_ENCODER;
		encoder.read_cb = Some(encoder_read);

		let encoder_ptr = unsafe { lv_indev_drv_register(encoder) };
		let group = unsafe { lv_group_create() };
		unsafe { lv_indev_set_group(encoder_ptr, group) };

		unsafe {
			//lv_group_add_obj(group, button1.raw().as_ptr());
			//lv_group_add_obj(group, button2.raw().as_ptr());
			lv_group_set_default(group);
		}

		Ok(Self {
			display:    lvgl_display,
			page_stack: ArrayVec::new(),
			indev:      encoder_ptr,
			enc_clk:    clk,
			enc_sw:     sw
		})
	}

	pub(crate) fn push_page(&mut self, page: Unipage<'a>) {
		// todo fix this panic
		self.page_stack.push(Box::new(page));

		// As long as the top page isn't popped without moving the active screen down
		// first i think this should be fine
		let new_top_screen = unsafe {
			self.page_stack
				.as_mut_ptr()
				.add(self.page_stack.len() - 1)
				.as_mut()
				.expect("Page stack pointer is null")
		};

		new_top_screen.make_group_active(self.indev);
		self.display.set_scr_act(new_top_screen.screen());
	}

	pub(crate) fn pop_page(&mut self) -> Option<Unipage<'a>> {
		assert_ne!(self.page_stack.len(), 1, "Attempting to pop main screen");

		let new_top_screen = unsafe {
			self.page_stack
				.as_mut_ptr()
				.add(self.page_stack.len() - 2)
				.as_mut()
				.expect("Page stack pointer is null")
		};

		new_top_screen.make_group_active(self.indev);
		self.display.set_scr_act(new_top_screen.screen());

		let old_top_screen = self.page_stack.pop();
		old_top_screen.map(|page| *page)
	}
}

fn http_request_fulfiller(communication_manager: Arc<Mutex<CommunicationManager>>) {
	loop {
		if let Some((method, uri, headers, done)) = communication_manager
			.lock()
			.unwrap()
			.request_signal
			.try_take()
		{
			let mut comm = communication_manager.lock().unwrap();
			let hdrs: Vec<(&str, &str)> = headers
				.iter()
				.map(|(key, val)| (key.as_str(), val.as_str()))
				.collect();
			let req = comm
				.http
				.request(method, uri.as_str(), hdrs.as_slice())
				.unwrap();
			let mut response = req.submit().unwrap();

			let mut body = Vec::new();
			let mut total = 0;
			while let Ok(n) = response.read(&mut body[total..])
				&& n != 0
			{
				total += n;
			}

			comm.response_signal.signal((body, done));
		}
		sleep(Duration::from_millis(100));
	}
}

#[embassy_executor::task]
pub async fn run_lcd(
	mut communication_manager: Arc<Mutex<CommunicationManager<'static>>>,
	spi: SpiDriver<'static>,
	cs: AnyOutputPin<'static>,
	dc: AnyOutputPin<'static>,
	rst: AnyOutputPin<'static>,
	enc_clk: AnyInputPin<'static>,
	enc_dt: AnyInputPin<'static>,
	enc_sw: AnyInputPin<'static>
) -> ! {
	info!("Running LCD");
	lvgl::init();

	let theme = Box::leak(Box::new(ModernTheme::new()));

	let mut display = InteractableDisplay::new(&spi, cs, dc, rst, enc_clk, enc_dt, enc_sw)
		.unwrap_or_else(|err| panic!("Failed to create LCD ({err})"));

	let f_cm = communication_manager.clone();
	thread::spawn(move || http_request_fulfiller(f_cm));

	// Create main page
	let p_cm = communication_manager.clone();
	let page = Unipage::try_new(theme, &mut display, p_cm, main_page).unwrap();
	

	display.push_page(page);

	debug!("Starting LCD loop");
	let mut last = Instant::now();
	loop {
		let d_cm = communication_manager.clone();
		match select(
			Timer::after_millis(5),
			communication_manager.lock().unwrap().response_signal.wait()
		)
		.await
		{
			Either::Second((response, done)) => {
				let page = Unipage::try_new(
					theme,
					&mut display,
					d_cm,
					|wf, theme, display, communication_manager| done(response, wf, theme, display)
				)
				.unwrap();
				display.push_page(page);
			}
			_ => {}
		}

		lvgl::task_handler();

		display.enc_clk.enable_interrupt().unwrap();
		display.enc_sw.enable_interrupt().unwrap();

		let now = Instant::now();
		lvgl::tick_inc(now - last);
		last = now;
	}
}

unsafe extern "C" fn encoder_read(_drv: *mut _lv_indev_drv_t, data: *mut lv_indev_data_t) {
	let data = &mut *data;

	data.enc_diff = ENCODER_STATE.diff;
	ENCODER_STATE.diff = 0;
	data.state = ENCODER_STATE.pressed;
}

/*pub fn chunked_draw_from_bitmap_reader<D: DrawTarget<Color = Rgb565>>(
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
}*/
