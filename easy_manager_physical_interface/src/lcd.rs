use std::collections::VecDeque;
use std::ffi::{c_int, c_void};
use std::mem::transmute;
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::Context;
use arrayvec::ArrayVec;
use easy_manager_core::packets::ErrorPacket;
use embassy_time::Timer;
use embedded_svc::http::client::Client;
use esp_idf_hal::gpio::{
	AnyInputPin,
	AnyOutputPin,
	Input,
	InputPin,
	InterruptType,
	Level,
	OutputPin,
	PinDriver,
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
	lcd_rgb_element_order_t_LCD_RGB_ELEMENT_ORDER_BGR
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

use crate::communications::{HttpPromise, HttpWork, RfidPromise, RfidReader};
use crate::graphics::{ModernTheme, Unipage};
use crate::pages::home_page;

/// Groups the logic for drawing to and interacting with a display
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

		// Make encoder interrupts
		unsafe {
			clk.subscribe(move || {
				let level = match dt.get_level() {
					Level::High => -1,
					Level::Low => 1
				};
				ENCODER_STATE.diff += level;
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

		let panel_dev_config = esp_lcd_panel_dev_config_t {
			reset_gpio_num: rst.pin() as c_int,
			data_endian: lcd_rgb_data_endian_t_LCD_RGB_DATA_ENDIAN_LITTLE,
			bits_per_pixel: 16,
			__bindgen_anon_1: esp_lcd_panel_dev_config_t__bindgen_ty_1 {
				rgb_ele_order: lcd_rgb_element_order_t_LCD_RGB_ELEMENT_ORDER_BGR
			},
			..Default::default()
		};

		let mut io_handle = esp_lcd_panel_io_handle_t::default();
		let mut panel_handle = esp_lcd_panel_handle_t::default();

		// Create and initialise the LCD interface
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

		// Allocate LCD framebuffer memory and make LVGL draw adapter
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
		.context("failed to create LVGL display")?;

		// Make LVGL input device
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

/// This is the function the fulfiller thread is on to do HTTP requests
fn request_fulfiller(
	http_config: Configuration,
	req: Arc<Mutex<Option<Box<HttpWork>>>>,
	res: Arc<Mutex<Option<Result<(u16, Vec<u8>), ErrorPacket>>>>
) {
	let mut session_id = None;

	loop {
		sleep(Duration::from_millis(100));

		// Wait for old request to be taken
		if res.lock().unwrap().is_some() {
			continue;
		}

		let req = req.lock().unwrap().take();
		if let Some(req) = req {
			let mut http = Client::wrap(
				EspHttpConnection::new(&http_config).expect("Failed to create HTTP client")
			);
			// Do HTTP work and update session id if needed
			let data = req.call_once((&mut http, &session_id));
			let data = data.map(|(code, session, data)| {
				if let Some(new_session) = session {
					session_id = new_session;
				}
				(code, data)
			});

			// Put response
			*res.lock().unwrap() = Some(data);
		}
	}
}

/// Updates LVGL and promise tasks forever
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

	// Keep theme alive forever in the same memory location. This allows me to copy
	// local references and have them stay valid
	let theme = Box::leak(Box::new(ModernTheme::new()));

	// Same with the display
	let mut display = Box::leak(Box::new(
		InteractableDisplay::new(&lcd_spi, lcd_cs, dc, rst, enc_clk, enc_dt, enc_sw)
			.unwrap_or_else(|err| panic!("Failed to create LCD ({err})"))
	));

	// Create the "slots" that the fulfiller thread will use to return data and know
	// when and how to make requests
	let mut current_request_done = None;
	let http_request = Arc::new(Mutex::new(None));
	let http_response = Arc::new(Mutex::new(None));

	let fulfiller_request = http_request.clone();
	let fulfiller_response = http_response.clone();
	thread::Builder::new()
		.stack_size(8196)
		.spawn(move || request_fulfiller(http_config, fulfiller_request, fulfiller_response))
		.expect("Failed to create HTTP request fulfiller thread");

	// Make the RFID reader. This doesn't need a separate thread like WiFi because
	// it doesn't block to read and it is nearly instant
	let key_cb: Box<dyn Fn(&Uid, u8) -> MifareKey> = Box::new(|_uid, _block| [0xffu8; 6]);
	let mut rfid = RfidReader::new(rfid_spi, Some(rfid_cs), key_cb).unwrap();

	// Create main page
	let page = Unipage::try_new(theme, display, home_page).expect("Failed to make main page");

	display.push_page(page);

	debug!("Starting LCD loop");
	let mut last = Instant::now();
	loop {
		Timer::after_millis(5).await;

		let mut rfid_data = None;
		let mut rfid_wrote = Ok(false);

		// Do RFID work without taking
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

		// Only take if data was actually read and call promise
		if let Some(data) = rfid_data {
			match display.rfid_promise.take() {
				Some(RfidPromise::Read(_, promise)) => promise.call_once((data, &mut display)),
				None | Some(RfidPromise::Write(..)) => {
					error!("Invalid promise to run");
				}
			}
		}

		// Only take if data was written or had an error
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

		// Tell the fulfiller to do another request if it isn't busy
		if current_request_done.is_none() {
			if let Some(HttpPromise { work, done }) = display.http_promises.pop_front() {
				current_request_done = Some(done);
				*http_request.lock().unwrap() = Some(work);
			}
		}

		// Call promise if request is done
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

		// Step LVGL
		lvgl::task_handler();

		if let Err(err) = display.enc_clk.enable_interrupt() {
			error!("Failed to enable encoder CLK interrupt ({err})");
		}
		if let Err(err) = display.enc_sw.enable_interrupt() {
			error!("Failed to enable encoder button interrupt ({err})");
		}

		// Tell LVGL how long the update took
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
