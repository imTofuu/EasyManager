use std::cell::Cell;
use std::rc::Rc;

use cstr_core::{CString, cstr};
use easy_manager_core::packets::Packet;
use easy_manager_core::packets::get::GetUsersResponse;
use easy_manager_core::packets::post::{ObtainPermanentTokenRequest, ObtainPermanentTokenResponse};
use embedded_svc::http::Method;
use esp_idf_sys::esp_restart;
use log::error;
use lvgl::sys::{
	LV_STATE_CHECKED,
	LV_STATE_DISABLED,
	lv_obj_add_state,
	lv_obj_has_state,
	lv_state_t
};
use lvgl::widgets::{Btn, Label, List, Switch};
use lvgl::{Align, Event, LvResult, NativeObject, Obj, Screen, Widget};

use crate::graphics::{Theme, Unipage, WidgetFactory};
use crate::lcd::InteractableDisplay;

const fn percent(percent: u32) -> u32 { (1 << 13) | percent }

fn http_loading_screen(
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	_display: &mut InteractableDisplay
) -> LvResult<()> {
	let mut label = wf.create_widget(Label::create)?;
	theme.primary_label(&mut label);
	label.set_text_static(cstr!("Please wait"));
	label.set_width(percent(100));

	Ok(())
}

fn err_page(
	emsg: &str,
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	let mut title = wf.create_widget(Label::create)?;
	title.set_text_static(cstr!("Error:"));
	title.set_width(percent(100));
	theme.primary_label(&mut title);

	let mut message = wf.create_widget(Label::create)?;
	message.set_text(
		CString::new(emsg)
			.unwrap_or(CString::new("Failed to parse error message").unwrap())
			.as_c_str()
	);
	message.set_width(percent(100));
	theme.misc_label(&mut message);

	let mut ok_button = wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
		let mut label = wf.create_widget(Label::create)?;
		label.set_text_static(cstr!("Ok"));
		Ok(())
	})?;
	ok_button.set_width(percent(100));
	theme.primary_button(&mut ok_button);
	ok_button.on_event(|_, e| {
		if let Event::Clicked = e {
			display.pop_page();
		}
	})?;

	Ok(())
}

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

	let write_new_card_option = Rc::new(Cell::new(false));

	let mut list = wf.create_parent_widget(List::create, theme, |mut wf, theme| {
		let mut new_card_option_container =
			wf.create_parent_widget(Obj::create, theme, |mut wf, theme| {
				let mut label = wf.create_widget(Label::create)?;
				label.set_text_static(cstr!("Write to valid cards"));
				theme.misc_label(&mut label);
				label.set_align(Align::TopLeft, 0, 0);

				let mut switch = wf.create_widget(Switch::create)?;
				switch.set_align(Align::TopRight, 0, 0);
				theme.switch(&mut switch);

				let event_new_card_option = write_new_card_option.clone();
				switch.on_event(move |widget, e| {
					if let Event::ValueChanged = e {
						event_new_card_option.set(unsafe {
							lv_obj_has_state(widget.raw().as_ptr(), LV_STATE_CHECKED as lv_state_t)
						});
					}
				})?;

				Ok(())
			})?;
		theme.option_container(&mut new_card_option_container);
		new_card_option_container.set_width(percent(100));
		Ok(())
	})?;
	theme.list(&mut list);
	list.set_width(percent(100));

	let mut final_buttons_container =
		wf.create_parent_widget(Obj::create, theme, |mut wf, theme| {
			let mut user_button =
				wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
					let mut label = wf.create_widget(Label::create)?;
					label.set_text_static(cstr!("User"));
					Ok(())
				})?;
			theme.primary_button(&mut user_button);
			user_button.set_align(Align::RightMid, 0, 0);
			user_button.set_width(percent(45));
			user_button.on_event(|_, e| {
				if let Event::Clicked = e {
					if let Err(err) = Unipage::try_new_with_http(
						Method::Get,
						"https://api.easy.drewbryan.org/users".to_string(),
						None::<()>,
						vec![],
						theme,
						display,
						program_user_page,
						http_loading_screen
					) {
						error!("Failed to make page with http request ({err})");
						// todo move this inside the request
						display.pop_page();
					}
				}
			})?;

			Ok(())
		})?;
	theme.option_container(&mut final_buttons_container);
	final_buttons_container.set_width(percent(100));

	let mut cancel_button = wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
		let mut label = wf.create_widget(Label::create)?;
		label.set_text_static(cstr!("Cancel"));
		Ok(())
	})?;
	theme.secondary_button(&mut cancel_button);
	cancel_button.on_event(|_, e| {
		if let Event::Clicked = e {
			display.pop_page();
		}
	})?;
	cancel_button.set_align(Align::LeftMid, 0, 0);
	cancel_button.set_width(percent(100));

	Ok(())
}

fn program_user_page(
	packet: Packet<GetUsersResponse>,
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	match packet {
		Packet::Ok(packet) => {
			let mut title = wf.create_widget(Label::create)?;
			title.set_text_static(cstr!("Select user"));
			theme.primary_label(&mut title);

			let mut list = wf.create_parent_widget(List::create, theme, |mut wf, theme| {
				for user in packet.users {
					let mut failed_name = false;
					let mut user_button =
						wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
							let mut label = wf.create_widget(Label::create)?;
							let name = match CString::new(user.username.as_str()) {
								Ok(name) => name,
								Err(err) => {
									failed_name = true;
									CString::new(format!("Unknown ({err})")).unwrap()
								}
							};
							label.set_text(name.as_c_str());
							Ok(())
						})?;
					user_button.set_width(percent(100));
					theme.misc_button(&mut user_button);
					if failed_name {
						unsafe {
							lv_obj_add_state(
								user_button.raw().as_ptr(),
								LV_STATE_DISABLED as lv_state_t
							);
						}
					}

					let mut obtain_token = |user_id: &str| {
						if let Err(err) = Unipage::try_new_with_http(
							Method::Post,
							"https://api.easy.drewbryan.org/obtain_permanent_token".to_string(),
							Some(ObtainPermanentTokenRequest {
								user_id: user_id.to_string()
							}),
							vec![],
							theme,
							display,
							program_obtained_permanent_token,
							http_loading_screen
						) {
							error!("Failed to push page with obtained token ({err})");
							display.pop_page();
						}
					};

					let user_id = user.user_id;
					user_button.on_event(move |_, e| {
						if let Event::Clicked = e {
							obtain_token(user_id.as_str());
						}
					})?;
				}
				Ok(())
			})?;
			theme.list(&mut list);
			list.set_width(percent(100));
		}
		Packet::Error(err) => {
			err_page(err.message.as_str(), wf, theme, display)?;
		}
	}
	Ok(())
}

fn program_obtained_permanent_token(
	packet: Packet<ObtainPermanentTokenResponse>,
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	match packet {
		Packet::Ok(packet) => {
			let mut title = wf.create_widget(Label::create)?;
			title.set_text_static(cstr!("Scan card"));
			theme.primary_label(&mut title);

			Unipage::try_new_with_rfid_write(
				vec![
					(4, *b"USER\0\0\0\0\0\0\0\0\0\0\0\0"),
					(5, packet.token.to_ne_bytes()),
				],
				true,
				theme,
				display,
				|result, mut wf, theme, display| {
					let mut label = wf.create_widget(Label::create)?;
					label.set_text(match result {
						Ok(_) => cstr!("Success"),
						Err(err) => {
							error!("Failed to write to card ({err:?})");
							cstr!("Failed")
						}
					});
					theme.primary_label(&mut label);

					let mut ok_button =
						wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
							let mut label = wf.create_widget(Label::create)?;
							label.set_text_static(cstr!("Ok"));
							Ok(())
						})?;
					theme.primary_button(&mut ok_button);
					ok_button.on_event(|_, e| {
						if let Event::Clicked = e {
							// todo right now im just rebooting to get back to the main page
							unsafe { esp_restart() };
						}
					})?;

					Ok(())
				}
			)?;
		}
		Packet::Error(err) => {
			err_page(err.message.as_str(), wf, theme, display)?;
		}
	}

	Ok(())
}
