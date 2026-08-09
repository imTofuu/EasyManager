use std::cell::Cell;
use std::rc::Rc;

use cstr_core::{CString, cstr};
use easy_manager_core::packets::Packet;
use easy_manager_core::packets::post::{LoginResponse};
use esp_idf_hal::spi::SpiError;
use esp_idf_sys::esp_restart;
use log::{error, info};
use lvgl::sys::{LV_STATE_CHECKED, lv_obj_has_state, lv_state_t};
use lvgl::widgets::{Btn, Dropdown, Label, List, Switch};
use lvgl::{Align, Event, LvResult, NativeObject, Obj, Screen, Widget};
use mfrc522::Error;

use crate::graphics::{Theme, Unipage, WidgetFactory};
use crate::lcd::InteractableDisplay;

const fn percent(percent: u32) -> u32 { (1 << 13) | percent }

pub fn main_page(
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	let mut title = wf.create_widget(Label::create)?;
	theme.primary_label(&mut title);
	title.set_text_static(cstr!("EasyManager"));
	title.set_width(percent(100));

	let mut subtitle = wf.create_widget(Label::create)?;
	theme.secondary_label(&mut subtitle);
	subtitle.set_text_static(cstr!("Scan item or use menu"));
	subtitle.set_width(percent(100));

	let mut list = wf.create_parent_widget(List::create, theme, |mut wf, theme| {
		let mut borrow_button = wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
			let mut label = wf.create_widget(Label::create)?;
			label.set_text_static(cstr!("Borrow"));
			label.set_width(percent(100));
			Ok(())
		})?;
		borrow_button.set_width(percent(100));
		theme.primary_button(&mut borrow_button);
		borrow_button.on_event(|_, e| {
			if let Event::Clicked = e {
				match Unipage::try_new(theme, display, borrow_page) {
					Ok(page) => display.push_page(page),
					Err(err) => error!("Something went wrong creating the borrowing page ({err})")
				}
			}
		})?;

		let mut return_button = wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
			let mut label = wf.create_widget(Label::create)?;
			label.set_text_static(cstr!("Return"));
			label.set_width(percent(100));
			Ok(())
		})?;
		return_button.set_width(percent(100));
		theme.secondary_button(&mut return_button);

		let mut program_button =
			wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
				let mut label = wf.create_widget(Label::create)?;
				label.set_text_static(cstr!("Program"));
				label.set_width(percent(100));
				Ok(())
			})?;
		program_button.set_width(percent(100));
		theme.secondary_button(&mut program_button);
		let program_theme = *&theme;
		program_button.on_event(|_, e| {
			if let Event::Clicked = e {
				match Unipage::try_new(program_theme, display, program_page) {
					Ok(page) => display.push_page(page),
					Err(err) => error!("Something went wrong creating the programming page ({err})")
				}
			}
		})?;

		let mut settings_button =
			wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
				let mut label = wf.create_widget(Label::create)?;
				label.set_text_static(cstr!("Settings"));
				label.set_width(percent(100));
				Ok(())
			})?;
		settings_button.set_width(percent(100));
		theme.secondary_button(&mut settings_button);

		let mut reboot_button = wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
			let mut label = wf.create_widget(Label::create)?;
			label.set_text_static(cstr!("Reboot"));
			label.set_width(percent(100));
			Ok(())
		})?;
		reboot_button.set_width(percent(100));
		theme.misc_button(&mut reboot_button);
		reboot_button.on_event(|_, e| unsafe {
			// todo add safe exit
			if let Event::Clicked = e {
				esp_restart();
			}
		})?;

		Ok(())
	})?;
	list.set_width(percent(100));
	theme.list(&mut list);

	Ok(())
}

fn borrow_page(
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	let mut label = wf.create_widget(Label::create)?;
	theme.primary_label(&mut label);
	label.set_text_static(cstr!("Borrow page"));
	label.set_width(percent(100));

	let mut back_button = wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
		let mut label = wf.create_widget(Label::create)?;
		label.set_text_static(cstr!("Back"));
		Ok(())
	})?;
	theme.primary_button(&mut back_button);
	back_button.on_event(|_, e| {
		if let Event::Clicked = e {
			display.pop_page();
		}
	})?;

	Ok(())
}

fn program_page(
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	let mut label = wf.create_widget(Label::create)?;
	theme.primary_label(&mut label);
	label.set_text_static(cstr!("Program RFID"));
	label.set_width(percent(100));

	let default_write = Rc::new(Cell::new(false));

	let mut list = wf.create_parent_widget(List::create, theme, |mut wf, theme| {
		let mut dropdown_container =
			wf.create_parent_widget(Obj::create, theme, |mut wf, theme| {
				let mut label = wf.create_widget(Label::create)?;
				label.set_text_static(cstr!("Write type"));
				theme.misc_label(&mut label);
				label.set_align(Align::TopLeft, 0, 0);

				let mut dropdown = wf.create_widget(Dropdown::create)?;
				dropdown.set_options_static(cstr!("User\nItem"));
				dropdown.set_align(Align::TopRight, 0, 0);

				Ok(())
			})?;
		theme.option_container(&mut dropdown_container);
		dropdown_container.set_width(percent(100));

		let mut overwrite_container =
			wf.create_parent_widget(Obj::create, theme, |mut wf, theme| {
				let mut label = wf.create_widget(Label::create)?;
				label.set_text_static(cstr!("Write to valid cards"));
				theme.misc_label(&mut label);
				label.set_align(Align::TopLeft, 0, 0);

				let mut overwrite_switch = wf.create_widget(Switch::create)?;
				overwrite_switch.set_align(Align::TopRight, 0, 0);
				theme.switch(&mut overwrite_switch);

				let event_default_write = default_write.clone();
				overwrite_switch.on_event(move |widget, e| {
					if let Event::ValueChanged = e {
						event_default_write.set(unsafe {
							lv_obj_has_state(widget.raw().as_ptr(), LV_STATE_CHECKED as lv_state_t)
						});
					}
				})?;

				Ok(())
			})?;
		theme.option_container(&mut overwrite_container);
		overwrite_container.set_width(percent(100));
		Ok(())
	})?;
	theme.list(&mut list);
	list.set_width(percent(100));

	let mut final_buttons_container =
		wf.create_parent_widget(Obj::create, theme, |mut wf, theme| {
			let mut cancel_button =
				wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
					let mut label = wf.create_widget(Label::create)?;
					label.set_text_static(cstr!("Cancel"));
					Ok(())
				})?;
			theme.secondary_button(&mut cancel_button);
			cancel_button.on_event(|_, e| {
				/*if let Event::Clicked = e {
					display.pop_page();
				}*/

				if let Event::Clicked = e {
					Unipage::try_new_with_rfid_read(7..8, theme, display, test_rfid).unwrap();
				}
			})?;
			cancel_button.set_align(Align::LeftMid, 0, 0);
			cancel_button.set_width(percent(45));

			let mut program_button =
				wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
					let mut label = wf.create_widget(Label::create)?;
					label.set_text_static(cstr!("Program"));
					Ok(())
				})?;
			theme.primary_button(&mut program_button);
			program_button.on_event(move |_, e| {
				let default_write = default_write.clone();
				(|| {
				if let Event::Clicked = e {
					Unipage::try_new_with_rfid_write(
						vec![
							(6, *b"hello\0\0\0\0\0\0\0\0\0\0\0"),
							(7, *b"dont write\0\0\0\0\0\0"),
						],
						default_write.get(),
						theme,
						display,
						test_rfid_write
					)
					.unwrap();
					/*let headers = vec![];
					Unipage::try_new_with_http(
						Method::Post,
						"https://api.easy.drewbryan.org/login".into(),
						Some(LoginRequest {
							account_identifier: AccountIdentifier::Username {
								username: "admin".into()
							},
							password:           "admin".into()
						}),
						headers,
						theme,
						display,
						test
					)
					.unwrap();*/
				}
			})();
			})?;
			program_button.set_align(Align::RightMid, 0, 0);
			program_button.set_width(percent(45));

			Ok(())
		})?;
	theme.option_container(&mut final_buttons_container);
	final_buttons_container.set_width(percent(100));

	Ok(())
}

fn test(
	packet: Packet<LoginResponse>,
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	_display: &mut InteractableDisplay
) -> LvResult<()> {
	let packet = match packet {
		Packet::Ok(packet) => packet,
		Packet::Error(err) => {
			error!("err packet: ({})", err.message);
			return Ok(());
		}
	};
	let mut label = wf.create_widget(Label::create)?;
	label.set_text(
		CString::new(packet.session_id)
			.unwrap_or(cstr!("Invalid session id").into())
			.as_c_str()
	);
	theme.primary_label(&mut label);

	Ok(())
}

fn test_rfid(
	data: Result<Vec<[u8; 16]>, Error<SpiError>>,
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	_display: &mut InteractableDisplay
) -> LvResult<()> {
	let data = data.unwrap().concat();
	let mut label = wf.create_widget(Label::create)?;
	label.set_text(CString::new(data).unwrap().as_c_str());
	theme.primary_label(&mut label);
	Ok(())
}

fn test_rfid_write(
	result: Result<(), Error<SpiError>>,
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	_display: &mut InteractableDisplay
) -> LvResult<()> {
	Ok(())
}
