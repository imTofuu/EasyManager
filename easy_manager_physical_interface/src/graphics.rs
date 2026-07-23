use std::cell::UnsafeCell;

use lvgl::font::Font;
use lvgl::style::{FlexFlow, Layout, Opacity, Style};
use lvgl::sys::{
	lv_group_create,
	lv_group_del,
	lv_group_set_default,
	lv_group_t,
	lv_indev_set_group,
	lv_indev_t
};
use lvgl::widgets::{Btn, Label};
use lvgl::{Align, Color, LvResult, NativeObject, Part, Screen, Widget};

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

pub struct WidgetFactory<'p, P: NativeObject>(&'p mut P);
impl<'p, P: NativeObject> WidgetFactory<'p, P> {
	pub fn create_widget<'a, W: Widget<'a>>(
		&mut self,
		constr: impl FnOnce(&mut P) -> LvResult<W>
	) -> LvResult<W> {
		constr(self.0)
	}

	pub fn create_parent_widget<'a, W: Widget<'a>, T: Theme>(
		&mut self,
		constr: impl FnOnce(&mut P) -> LvResult<W>,
		theme: &'a T,
		init: impl FnOnce(WidgetFactory<W>, &'a T) -> LvResult<()>
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
		theme: &'a T,
		display: &mut InteractableDisplay<'a>,
		init: impl FnOnce(
			WidgetFactory<Screen<'a>>,
			&'a T,
			&mut InteractableDisplay<'a>
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
	// Swap blue and red when making colours
	fn dominant() -> Color;
	fn secondary() -> Color;
	fn accent() -> Color;
	fn text() -> Color;

	fn screen<'a>(&'a self, widget: &mut Screen<'a>);

	fn primary_label<'a>(&'a self, widget: &mut Label<'a>);

	fn primary_button<'a>(&'a self, widget: &mut Btn<'a>);
}

pub struct ModernTheme {
	pub screen: StyleCell,

	pub primary_label:   StyleCell,
	pub secondary_label: StyleCell,
	pub misc_label:      StyleCell,

	pub primary_button:   StyleCell,
	pub secondary_button: StyleCell,
	pub misc_button:      StyleCell
}

impl Theme for ModernTheme {
	fn dominant() -> Color { Color::from_rgb((0xe9, 0xf5, 0xfc)) }
	fn secondary() -> Color { Color::from_rgb((0x10, 0x50, 0x70)) }
	fn accent() -> Color { Color::from_rgb((0x20, 0x9f, 0xdf)) }
	fn text() -> Color { Color::from_rgb((0x08, 0x0e, 0x11)) }

	fn screen<'a>(&'a self, widget: &mut Screen<'a>) { self.screen.apply(Part::Main, widget); }

	fn primary_label<'a>(&'a self, widget: &mut Label<'a>) {
		self.primary_label.apply(Part::Main, widget);
	}

	fn primary_button<'a>(&'a self, widget: &mut Btn<'a>) {
		self.primary_button.apply(Part::Main, widget);
	}
}

impl ModernTheme {
	pub fn new() -> Self {
		Self {
			screen: {
				let mut style = Style::default();
				style.set_bg_color(Self::dominant());
				style.set_layout(Layout::flex());
				style.set_flex_flow(FlexFlow::COLUMN);
				style.set_flex_grow(1);
				style.set_align(Align::Center);
				StyleCell::new(style)
			},

			primary_label:   {
				let mut style = Style::default();
				style.set_text_color(Self::accent());
				style.set_text_font(Font::montserrat_40());
				StyleCell::new(style)
			},
			secondary_label: {
				let mut style = Style::default();
				style.set_text_color(Self::secondary());
				StyleCell::new(style)
			},
			misc_label:      {
				let mut style = Style::default();
				style.set_text_color(Self::text());
				StyleCell::new(style)
			},

			primary_button:   {
				let mut style = Style::default();
				style.set_bg_color(Self::accent());
				style.set_text_color(Self::dominant());
				StyleCell::new(style)
			},
			secondary_button: {
				let mut style = Style::default();
				style.set_bg_opa(Opacity::empty());
				style.set_text_color(Self::accent());
				style.set_outline_width(3);
				style.set_outline_color(Self::accent());
				StyleCell::new(style)
			},
			misc_button:      {
				let mut style = Style::default();
				style.set_text_color(Self::text());
				StyleCell::new(style)
			}
		}
	}
}
