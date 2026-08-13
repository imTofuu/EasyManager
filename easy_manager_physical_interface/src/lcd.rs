use std::collections::VecDeque;
use std::ffi::{c_int, c_void};
use std::io::Read;
use std::mem::transmute;
use std::sync::atomic::{AtomicI16, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::Context;
use arrayvec::ArrayVec;
use embassy_time::Timer;
use embedded_sdmmc::{BlockDevice, File, TimeSource, Timestamp};
use embedded_svc::http::client::Client;
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
use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
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

use crate::communications::{HttpPromise, HttpWork, HttpWorkResult, RfidPromise, RfidReader};
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
	#[allow(clippy::unwrap_used)]
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

pub struct InteractableDisplay {
	display:    Display,
	page_stack: ArrayVec<Box<Unipage<'static>>, 64>,
	indev:      *mut lv_indev_t,

	http_promises: VecDeque<HttpPromise>,
	rfid_promise:  Option<RfidPromise>,

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

impl InteractableDisplay {
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
			lv_group_set_default(group);
		}

		Ok(Self {
			display:       lvgl_display,
			page_stack:    ArrayVec::new(),
			http_promises: VecDeque::new(),
			rfid_promise:  None,
			indev:         encoder_ptr,
			enc_clk:       clk,
			enc_sw:        sw
		})
	}

	pub fn add_http_promise(&mut self, promise: HttpPromise) {
		self.http_promises.push_back(promise);
	}

	pub fn set_rfid_promise(&mut self, promise: Option<RfidPromise>) -> Option<RfidPromise> {
		let old = self.rfid_promise.take();
		self.rfid_promise = promise;
		old
	}

	pub fn push_page(&mut self, page: Unipage) {
		// todo fix this panic
		// Erases the lifetime parameter of the page because the children shouldn't get
		// dropped until the page is dropped as long as lv_obj_del is never manually
		// called
		self.page_stack.push(Box::new(unsafe { transmute(page) }));

		// As long as the active screen is changed before this is popped it should be
		// fine
		let new_top_screen = unsafe {
			self.page_stack
				.as_mut_ptr()
				.add(self.page_stack.len() - 1)
				.as_mut_unchecked()
		};

		new_top_screen.make_group_active(self.indev);
		self.display.set_scr_act(new_top_screen.screen());
	}

	pub fn pop_page<'a>(&mut self) -> Option<Box<Unipage<'a>>> {
		//assert_ne!(self.page_stack.len(), 1, "Attempting to pop main screen");
		if self.page_stack.len() == 1 {
			return None;
		}

		let new_top_screen = unsafe {
			self.page_stack
				.as_mut_ptr()
				.add(self.page_stack.len() - 2)
				.as_mut_unchecked()
		};

		new_top_screen.make_group_active(self.indev);
		self.display.set_scr_act(new_top_screen.screen());

		self.page_stack.pop()
	}
}

fn request_fulfiller(
	http_config: Configuration,
	req: Arc<Mutex<Option<Box<HttpWork>>>>,
	res: Arc<Mutex<Option<HttpWorkResult>>>
) {
	let mut http =
		Client::wrap(EspHttpConnection::new(&http_config).expect("Failed to create HTTP client"));

	loop {
		sleep(Duration::from_millis(100));
		if res.lock().unwrap().is_some() {
			continue;
		}
		let req = req.lock().unwrap().take();
		if let Some(req) = req {
			let data = req.call_once((&mut http,));
			*res.lock().unwrap() = Some(data);
		}
	}
}

#[embassy_executor::task]
pub async fn run_lcd(
	http_config: Configuration,
	rfid_spi: SpiDriver<'static>,
	rfid_cs: AnyOutputPin<'static>,
	lcd_spi: SpiDriver<'static>,
	lcd_cs: AnyOutputPin<'static>,
	dc: AnyOutputPin<'static>,
	rst: AnyOutputPin<'static>,
	enc_clk: AnyInputPin<'static>,
	enc_dt: AnyInputPin<'static>,
	enc_sw: AnyInputPin<'static>
) -> ! {
	info!("Running LCD");
	lvgl::init();

	let theme = Box::leak(Box::new(ModernTheme::new()));

	let mut display = Box::leak(Box::new(
		InteractableDisplay::new(&lcd_spi, lcd_cs, dc, rst, enc_clk, enc_dt, enc_sw)
			.unwrap_or_else(|err| panic!("Failed to create LCD ({err})"))
	));

	let mut current_request_done = None;
	let http_request = Arc::new(Mutex::new(None));
	let http_response = Arc::new(Mutex::new(None));

	let fulfiller_request = http_request.clone();
	let fulfiller_response = http_response.clone();
	thread::Builder::new()
		.stack_size(8196)
		.spawn(move || request_fulfiller(http_config, fulfiller_request, fulfiller_response))
		.expect("Failed to create HTTP request fulfiller thread");

	let key_cb: Box<dyn Fn(&Uid, u8) -> MifareKey> = Box::new(|_uid, _block| [0xffu8; 6]);
	let mut rfid = RfidReader::new(rfid_spi, Some(rfid_cs), key_cb).unwrap();

	// Create main page
	let page = Unipage::try_new(theme, display, main_page).expect("Failed to create main page");

	display.push_page(page);

	debug!("Starting LCD loop");
	let mut last = Instant::now();
	loop {
		Timer::after_millis(5).await;

		let mut rfid_data = None;
		let mut rfid_wrote = Ok(false);

		match &display.rfid_promise {
			Some(RfidPromise::Read(work, _)) => {
				if let Some(data) = work.call((&mut rfid,)).transpose() {
					rfid_data = Some(data)
				}
			}
			Some(RfidPromise::Write(work, _)) => {
				rfid_wrote = work.call((&mut rfid,));
			}
			None => {}
		}

		if let Some(data) = rfid_data {
			match display.rfid_promise.take() {
				Some(RfidPromise::Read(_, promise)) => promise.call_once((data, &mut display)),
				None | Some(RfidPromise::Write(..)) => {
					error!("Invalid promise to run");
				}
			}
		}

		if let result @ Ok(true) | result @ Err(_) = rfid_wrote {
			match display.rfid_promise.take() {
				Some(RfidPromise::Write(_, promise)) => {
					promise.call_once((result.map(|_| ()), &mut display))
				}
				None | Some(RfidPromise::Read(..)) => {
					error!("Invalid promise to run");
				}
			}
		}

		if current_request_done.is_none() {
			if let Some(HttpPromise { work, done }) = display.http_promises.pop_front() {
				current_request_done = Some(done);
				*http_request.lock().unwrap() = Some(work);
			}
		}

		{
			if let Some(res) = http_response.lock().unwrap().take() {
				*http_request.lock().unwrap() = None;
				match current_request_done.take() {
					Some(promise) => {
						promise.call_once((res, display));
					}
					None => {
						error!("HTTP response with no request");
						debug_assert!(false);
					}
				}
			}
		}

		lvgl::task_handler();

		if let Err(err) = display.enc_clk.enable_interrupt() {
			error!("Failed to enable encoder CLK interrupt ({err})");
		}
		if let Err(err) = display.enc_sw.enable_interrupt() {
			error!("Failed to enable encoder button interrupt ({err})");
		}

		let now = Instant::now();
		lvgl::tick_inc(now - last);
		last = now;
	}
}

unsafe extern "C" fn encoder_read(_drv: *mut _lv_indev_drv_t, data: *mut lv_indev_data_t) {
	unsafe {
		let data = &mut *data;

		data.enc_diff = ENCODER_STATE.diff;
		ENCODER_STATE.diff = 0;
		data.state = ENCODER_STATE.pressed;
	}
}
