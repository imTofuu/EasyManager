use std::cell::UnsafeCell;
use std::ops::Range;

use easy_manager_core::packets::Packet;
use embedded_svc::http::Method;
use esp_idf_hal::spi::SpiError;
use log::error;
use lvgl::font::Font;
use lvgl::misc::area::LV_SIZE_CONTENT;
use lvgl::style::{FlexAlign, FlexFlow, Layout, Opacity, Style};
use lvgl::sys::{
	LV_PART_INDICATOR,
	LV_PART_MAIN,
	LV_STATE_CHECKED,
	LV_STATE_FOCUSED,
	lv_group_create,
	lv_group_del,
	lv_group_set_default,
	lv_group_t,
	lv_indev_set_group,
	lv_indev_t,
	lv_obj_add_style,
	lv_opa_t,
	lv_style_init,
	lv_style_set_bg_color,
	lv_style_set_bg_opa,
	lv_style_set_outline_color,
	lv_style_set_text_color,
	lv_style_t
};
use lvgl::widgets::{Btn, Label, List, Switch};
use lvgl::{Color, LvResult, NativeObject, Obj, Part, Screen, TextAlign, Widget};
use mfrc522::Error;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::communications::{HttpPromise, RfidPromise};
use crate::lcd::InteractableDisplay;

pub struct StyleCell {
	inner: UnsafeCell<Style>
}

impl StyleCell {
	pub fn new(style: Style) -> Self {
		Self {
			inner: UnsafeCell::new(style)
		}
	}

	pub fn apply<'a, P, W: Widget<'a, Part = P>>(&'a self, part: P, widget: &mut W) {
		// SAFETY: The style is owned by StyleCell which means that rust can no longer
		// mutate it, only LVGL
		widget.add_style(part, unsafe { self.inner.as_mut_unchecked() });
	}
}

pub struct RawStyleCell {
	inner: UnsafeCell<lv_style_t>
}

impl RawStyleCell {
	pub fn new() -> Self {
		let mut style = lv_style_t::default();
		unsafe {
			lv_style_init(&mut style as *mut lv_style_t);
		}
		Self {
			inner: UnsafeCell::new(style)
		}
	}

	pub unsafe fn inner(&self) -> *mut lv_style_t {
		unsafe { self.inner.as_mut_unchecked() as *mut lv_style_t }
	}
}

pub struct WidgetFactory<'p, P: NativeObject + 'p>(&'p mut P);
impl<'p, P: NativeObject> WidgetFactory<'p, P> {
	pub fn create_widget<'a, W: Widget<'a>>(
		&'a mut self,
		constr: impl FnOnce(&'a mut P) -> LvResult<W>
	) -> LvResult<W> {
		constr(self.0)
	}

	pub fn create_parent_widget<'a, W: Widget<'a>, T: Theme>(
		&'a mut self,
		constr: impl FnOnce(&'a mut P) -> LvResult<W>,
		theme: &'static T,
		init: impl FnOnce(WidgetFactory<W>, &'static T) -> LvResult<()>
	) -> LvResult<W> {
		let mut w = self.create_widget(constr)?;
		let wf = WidgetFactory(&mut w);
		init(wf, theme)?;
		Ok(w)
	}
}

pub struct Unipage<'a> {
	inner: Screen<'a>,
	group: *mut lv_group_t
}

impl<'a> Unipage<'a> {
	/// It is UB to drop the Unipage instance from init
	pub fn try_new<T: Theme>(
		theme: &'static T,
		display: &mut InteractableDisplay,
		init: impl FnOnce(
			WidgetFactory<Screen<'a>>,
			&'static T,
			&mut InteractableDisplay
		) -> LvResult<()>
	) -> LvResult<Self> {
		let mut screen = Screen::blank()?;
		theme.screen(&mut screen);

		let group = unsafe { lv_group_create() };
		unsafe { lv_group_set_default(group) };

		let wf = WidgetFactory(&mut screen);
		init(wf, theme, display)?;

		Ok(Self {
			inner: screen,
			group
		})
	}

	// THIS AUTOMATICALLY PUSHES THE PAGE; i will change it at some point
	pub fn try_new_with_http<
		T: Theme,
		B: Serialize + Send + 'static,
		R: Serialize + DeserializeOwned + Send + 'static
	>(
		method: Method,
		uri: String,
		body: Option<B>,
		headers: Vec<(String, String)>,
		theme: &'static T,
		display: &mut InteractableDisplay,
		init: impl FnOnce(
			Packet<R>,
			WidgetFactory<Screen>,
			&'static T,
			&mut InteractableDisplay
		) -> LvResult<()>
		+ 'static,
		loading_screen: impl FnOnce(
			WidgetFactory<Screen>,
			&'static T,
			&mut InteractableDisplay
		) -> LvResult<()>
	) -> LvResult<()> {
		let loading_screen_page = Self::try_new(theme, display, loading_screen)?;
		display.push_page(loading_screen_page);
		let promise = HttpPromise::new(
			method,
			uri,
			body,
			headers,
			move |packet: Packet<R>, closure_display| {
				let page = match Self::try_new(theme, closure_display, |wf, thm, inner_display| {
					init(packet, wf, thm, inner_display)?;
					Ok(())
				}) {
					Ok(page) => page,
					Err(err) => {
						error!("Failed to create page with HTTP request ({err})");
						return;
					}
				};
				closure_display.pop_page();
				closure_display.pop_page();
				closure_display.push_page(page);
			}
		);
		display.add_http_promise(promise);
		Ok(())
	}

	// THIS AUTOMATICALLY PUSHES THE PAGE; i will change it at some point
	pub fn try_new_with_rfid_read<T: Theme>(
		blocks: Range<u8>,
		theme: &'static T,
		display: &mut InteractableDisplay,
		init: impl FnOnce(
			Result<Vec<[u8; 16]>, Error<SpiError>>,
			WidgetFactory<Screen>,
			&'static T,
			&mut InteractableDisplay
		) -> LvResult<()>
		+ 'static
	) -> LvResult<()> {
		let promise = RfidPromise::read(
			blocks,
			Box::new(move |data, display| {
				let page = match Self::try_new(theme, display, |wf, theme, display| {
					init(data, wf, theme, display)?;
					Ok(())
				}) {
					Ok(page) => page,
					Err(err) => {
						error!("Failed to create page with RFID read request ({err})");
						return;
					}
				};
				display.pop_page();
				display.push_page(page);
			})
		);
		display.set_rfid_promise(Some(promise));
		Ok(())
	}

	pub fn try_new_with_rfid_write<T: Theme>(
		data: Vec<(u8, [u8; 16])>,
		default: bool,
		theme: &'static T,
		display: &mut InteractableDisplay,
		init: impl FnOnce(
			Result<(), Error<SpiError>>,
			WidgetFactory<Screen>,
			&'static T,
			&mut InteractableDisplay
		) -> LvResult<()>
		+ 'static
	) -> LvResult<()> {
		let promise = RfidPromise::write(
			data,
			default,
			Box::new(move |result, display| {
				let page = match Self::try_new(theme, display, |wf, theme, display| {
					init(result, wf, theme, display)?;
					Ok(())
				}) {
					Ok(page) => page,
					Err(err) => {
						error!("Failed to create page with RFID write request ({err})");
						return;
					}
				};
				display.pop_page();
				display.push_page(page);
			})
		);
		display.set_rfid_promise(Some(promise));
		Ok(())
	}

	// Shouldn't be called normally, will be removed at some point
	#[deprecated]
	pub fn screen(&mut self) -> &'a mut Screen<'_> { &mut self.inner }
	pub fn make_group_active(&self, indev: *mut lv_indev_t) {
		unsafe { lv_indev_set_group(indev, self.group) }
	}
}

impl<'a> Drop for Unipage<'a> {
	fn drop(&mut self) {
		unsafe { lv_group_del(self.group) };
		// SAFETY: LVGL rust bindings don't implement drop on any of the Obj types
		// because it is possible that the references (that look owned from rust) have
		// been dropped by a parent or other reference. As long as references are only
		// obtained by objects that belong to Unipage, no dangling references will
		// ever exist.
		unsafe { lvgl::sys::lv_obj_del(self.inner.raw().as_mut()) }
	}
}

pub trait Theme {
	fn dominant(&self) -> Color;
	fn secondary(&self) -> Color;
	fn accent(&self) -> Color;
	fn text(&self) -> Color;

	fn screen<'a>(&'a self, widget: &mut Screen<'a>);
	fn list<'a>(&'a self, widget: &mut List<'a>);
	fn option_container<'a>(&'a self, widget: &mut Obj<'a>);
	fn switch<'a>(&'a self, widget: &mut Switch<'a>);

	fn primary_label<'a>(&'a self, widget: &mut Label<'a>);
	fn secondary_label<'a>(&'a self, widget: &mut Label<'a>);
	fn misc_label<'a>(&'a self, widget: &mut Label<'a>);

	fn primary_button<'a>(&'a self, widget: &mut Btn<'a>);
	fn secondary_button<'a>(&'a self, widget: &mut Btn<'a>);
	fn misc_button<'a>(&'a self, widget: &mut Btn<'a>);
}

pub struct ModernTheme {
	pub dominant:  Color,
	pub secondary: Color,
	pub accent:    Color,
	pub text:      Color,

	pub screen: StyleCell,

	pub list: StyleCell,

	pub option_container: StyleCell,

	pub switch:           StyleCell,
	//pub switch_hover: RawStyleCell,
	pub switch_indicator: RawStyleCell,
	pub switch_knob:      StyleCell,

	pub primary_label:   StyleCell,
	pub secondary_label: StyleCell,
	pub misc_label:      StyleCell,

	pub primary_button:   StyleCell,
	pub secondary_button: StyleCell,
	pub misc_button:      StyleCell,
	pub button_hover:     RawStyleCell
}

impl Theme for ModernTheme {
	fn dominant(&self) -> Color { self.dominant }
	fn secondary(&self) -> Color { self.secondary }
	fn accent(&self) -> Color { self.accent }
	fn text(&self) -> Color { self.text }

	fn screen<'a>(&'a self, widget: &mut Screen<'a>) { self.screen.apply(Part::Main, widget); }

	fn list<'a>(&'a self, widget: &mut List<'a>) { self.list.apply(Part::Main, widget); }

	fn option_container<'a>(&'a self, widget: &mut Obj<'a>) {
		self.option_container.apply(Part::Main, widget);
	}

	fn switch<'a>(&'a self, widget: &mut Switch<'a>) {
		self.switch.apply(Part::Main, widget);
		self.switch_knob.apply(Part::Knob, widget);
		unsafe {
			lv_obj_add_style(
				widget.raw().as_ptr(),
				self.switch_indicator.inner(),
				LV_PART_INDICATOR | LV_STATE_CHECKED
			);
		}
	}

	fn primary_label<'a>(&'a self, widget: &mut Label<'a>) {
		self.primary_label.apply(Part::Main, widget);
	}

	fn secondary_label<'a>(&'a self, widget: &mut Label<'a>) {
		self.secondary_label.apply(Part::Main, widget);
	}

	fn misc_label<'a>(&'a self, widget: &mut Label<'a>) {
		self.misc_label.apply(Part::Main, widget);
	}

	fn primary_button<'a>(&'a self, widget: &mut Btn<'a>) {
		self.primary_button.apply(Part::Main, widget);
		unsafe {
			lv_obj_add_style(
				widget.raw().as_ptr(),
				self.button_hover.inner(),
				LV_PART_MAIN | LV_STATE_FOCUSED
			);
		}
	}

	fn secondary_button<'a>(&'a self, widget: &mut Btn<'a>) {
		self.secondary_button.apply(Part::Main, widget);
		unsafe {
			lv_obj_add_style(
				widget.raw().as_ptr(),
				self.button_hover.inner(),
				LV_PART_MAIN | LV_STATE_FOCUSED
			);
		}
	}

	fn misc_button<'a>(&'a self, widget: &mut Btn<'a>) {
		self.misc_button.apply(Part::Main, widget);
		unsafe {
			lv_obj_add_style(
				widget.raw().as_ptr(),
				self.button_hover.inner(),
				LV_PART_MAIN | LV_STATE_FOCUSED
			);
		}
	}
}

impl ModernTheme {
	pub fn new() -> Self {
		let dominant = Color::from_rgb((0xe9, 0xf5, 0xfc));
		let secondary = Color::from_rgb((0x10, 0x50, 0x70));
		let accent = Color::from_rgb((0x20, 0x9f, 0xdf));
		let text = Color::from_rgb((0x08, 0x0e, 0x11));
		Self {
			dominant,
			secondary,
			accent,
			text,

			screen: {
				let mut style = Style::default();
				style.set_bg_color(dominant);
				style.set_layout(Layout::flex());
				style.set_flex_flow(FlexFlow::COLUMN);
				style.set_flex_main_place(FlexAlign::START);
				style.set_flex_cross_place(FlexAlign::CENTER);
				style.set_pad_left(5);
				style.set_pad_right(5);
				StyleCell::new(style)
			},

			list: {
				let mut style = Style::default();
				style.set_bg_color(dominant);
				style.set_height(LV_SIZE_CONTENT as i16);
				style.set_pad_top(5);
				style.set_pad_left(5);
				style.set_pad_bottom(5);
				style.set_pad_right(5);
				style.set_pad_row(10);
				StyleCell::new(style)
			},

			option_container: {
				let mut style = Style::default();
				style.set_flex_flow(FlexFlow::ROW_WRAP);
				style.set_border_opa(Opacity::OPA_0);
				style.set_bg_opa(Opacity::OPA_0);
				style.set_height(LV_SIZE_CONTENT as i16);
				style.set_pad_top(5);
				style.set_pad_left(5);
				style.set_pad_bottom(5);
				style.set_pad_right(5);
				StyleCell::new(style)
			},

			switch: {
				let mut style = Style::default();
				style.set_bg_color(text);
				StyleCell::new(style)
			},
			switch_indicator: {
				let style = RawStyleCell::new();
				unsafe {
					let ptr = style.inner();

					lv_style_set_bg_color(ptr, accent.into());
				}
				style
			},
			switch_knob: {
				let mut style = Style::default();
				style.set_shadow_opa(Opacity::OPA_40);
				style.set_shadow_width(16);
				style.set_shadow_ofs_y(2);
				StyleCell::new(style)
			},

			primary_label: {
				let mut style = Style::default();
				style.set_text_color(accent);
				style.set_text_font(Font::montserrat_40());
				style.set_text_align(TextAlign::Center);
				StyleCell::new(style)
			},
			secondary_label: {
				let mut style = Style::default();
				style.set_text_color(secondary);
				style.set_text_font(Font::montserrat_20());
				style.set_text_align(TextAlign::Center);
				StyleCell::new(style)
			},
			misc_label: {
				let mut style = Style::default();
				style.set_text_color(text);
				StyleCell::new(style)
			},

			primary_button: {
				let mut style = Style::default();
				style.set_bg_color(accent);
				style.set_text_color(dominant);
				style.set_text_align(TextAlign::Center);
				StyleCell::new(style)
			},
			secondary_button: {
				let mut style = Style::default();
				style.set_bg_opa(Opacity::OPA_0);
				style.set_text_color(secondary);
				style.set_outline_width(3);
				style.set_outline_color(secondary);
				style.set_text_align(TextAlign::Center);
				StyleCell::new(style)
			},
			misc_button: {
				let mut style = Style::default();
				style.set_bg_opa(Opacity::OPA_0);
				style.set_text_color(accent);
				style.set_shadow_opa(Opacity::OPA_0);
				style.set_text_align(TextAlign::Center);
				StyleCell::new(style)
			},
			button_hover: {
				let style = RawStyleCell::new();
				unsafe {
					let ptr = style.inner();

					lv_style_set_bg_opa(ptr, lv_opa_t::MAX);
					lv_style_set_bg_color(ptr, secondary.into());
					lv_style_set_text_color(ptr, dominant.into());
					lv_style_set_outline_color(ptr, secondary.into());
				}
				style
			}
		}
	}
}
