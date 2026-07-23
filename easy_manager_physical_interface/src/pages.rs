use cstr_core::cstr;
use lvgl::Align::Center;
use lvgl::widgets::{Btn, Label, List};
use lvgl::{Event, LvResult, Screen, Widget};

use crate::graphics::{Theme, Unipage, WidgetFactory};
use crate::lcd::InteractableDisplay;

pub fn main_page<'a>(
	mut wf: WidgetFactory<Screen<'_>>,
	theme: &'a impl Theme,
	display: &mut InteractableDisplay<'a>
) -> LvResult<()> {
	let mut title = wf.create_widget(Label::create)?;
	theme.primary_label(&mut title);
	title.set_text_static(cstr!("EasyManager"));
	title.set_align(Center, 0, 0);

	let _list = wf.create_parent_widget(List::create, theme, |mut wf, theme| {
		let mut borrow_button = wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
			let mut label = wf.create_widget(Label::create)?;
			label.set_text_static(cstr!("Borrow"));
			Ok(())
		})?;
		theme.primary_button(&mut borrow_button);
		let mut new_page_result: LvResult<()> = Ok(());
		borrow_button.on_event(|_, e| {
			if let Event::Clicked = e {
				match Unipage::try_new(theme, display, borrow_page) {
					Ok(page) => display.push_page(page),
					Err(err) => new_page_result = Err(err)
				}
			}
		})?;
		new_page_result?;

		let mut return_button = wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
			let mut label = wf.create_widget(Label::create)?;
			label.set_text_static(cstr!("Return"));
			Ok(())
		})?;
		theme.primary_button(&mut return_button);

		Ok(())
	})?;

	Ok(())
}

fn borrow_page(
	mut wf: WidgetFactory<Screen<'_>>,
	theme: &impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	let mut label = wf.create_widget(Label::create)?;
	theme.primary_label(&mut label);
	label.set_text_static(cstr!("Borrow page"));

	let mut back_button = wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
		let mut label = wf.create_widget(Label::create)?;
		label.set_text_static(cstr!("Cancel"));
		Ok(())
	})?;
	theme.primary_button(&mut back_button);
	back_button.on_event(|_, e| {
		if let Event::Clicked = e {
			display.pop_page();
			return;
		}
	})?;

	Ok(())
}
