use std::cell::Cell;
use std::rc::Rc;

use cstr_core::{CString, cstr};
use easy_manager_core::packets::Packet;
use easy_manager_core::packets::get::{
	GetItemInfoResponse,
	GetItemsResponse,
	GetLoggedInUserRequest,
	GetUserInfoResponse,
	GetUsersResponse
};
use easy_manager_core::packets::post::{
	BorrowRequest,
	LoginResponse,
	LoginUsingPermanentTokenRequest,
	ObtainPermanentTokenRequest,
	ObtainPermanentTokenResponse,
	ReturnRequest
};
use embedded_svc::http::Method;
use esp_idf_sys::esp_restart;
use log::{error, info};
use lvgl::misc::area::LV_SIZE_CONTENT;
use lvgl::sys::{
	LV_STATE_CHECKED,
	LV_STATE_DISABLED,
	lv_obj_add_state,
	lv_obj_has_state,
	lv_state_t
};
use lvgl::widgets::{Btn, Label, List, Switch};
use lvgl::{Align, Event, LvResult, NativeObject, Obj, Screen, Widget};
use uuid::Uuid;

use crate::graphics::{Theme, Unipage, WidgetFactory};
use crate::lcd::InteractableDisplay;

const USER_TYPE: [u8; 16] = *b"USER\0\0\0\0\0\0\0\0\0\0\0\0";
const ITEM_TYPE: [u8; 16] = *b"ITEM\0\0\0\0\0\0\0\0\0\0\0\0";

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
	label.set_align(Align::TopMid, 0, 0);

	Ok(())
}

fn err_page(
	code: u16,
	emsg: &str,
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	let mut title = wf.create_widget(Label::create)?;
	title.set_text_static(cstr!("Error:"));
	title.set_width(percent(100));
	theme.primary_label(&mut title);

	let mut code_label = wf.create_widget(Label::create)?;
	code_label.set_text(
		CString::new(code.to_string().as_str())
			.unwrap_or(CString::new("Failed to parse error message").unwrap())
			.as_c_str()
	);
	code_label.set_width(percent(100));
	theme.secondary_label(&mut code_label);

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
		unsafe {
			lv_obj_add_state(
				settings_button.raw().as_ptr(),
				LV_STATE_DISABLED as lv_state_t
			);
		}

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
	let mut title = wf.create_widget(Label::create)?;
	theme.primary_label(&mut title);
	title.set_text_static(cstr!("Borrow / Return"));
	title.set_width(percent(100));

	let mut subtitle = wf.create_widget(Label::create)?;
	subtitle.set_text_static(cstr!("Scan item"));
	subtitle.set_width(percent(100));
	theme.secondary_label(&mut subtitle);

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

	Unipage::try_new_with_rfid_read(4..6, theme, display, |data, mut wf, theme, display| {
		match data {
			Ok(data) => {
				if let Some(data_type) = data.get(0) {
					if *data_type == USER_TYPE {
						return err_page(
							0,
							"error: invalid card type (expected item, got user)",
							wf,
							theme,
							display
						);
					}

					let mut title = wf.create_widget(Label::create)?;
					title.set_text_static(cstr!("Please wait"));
					title.set_width(percent(100));
					title.set_align(Align::TopMid, 0, 0);
					theme.primary_label(&mut title);

					let item_id = Uuid::from_u128(u128::from_ne_bytes(
						data.get(1).cloned().unwrap_or([0u8; 16])
					))
					.to_string();

					Unipage::try_new_with_http(
						Method::Get,
						format!("https://api.easy.drewbryan.org/item/{item_id}"),
						None::<()>,
						vec![],
						theme,
						display,
						scan_user_for_borrow_page,
						http_loading_screen
					)?;
				}
			}
			Err(err) => {
				err_page(0, format!("error: {err:?}").as_str(), wf, theme, display)?;
			}
		}

		Ok(())
	})?;

	Ok(())
}

fn scan_user_for_borrow_page(
	packet: Packet<GetItemInfoResponse>,
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	match packet {
		Packet::Ok(_, packet) => {
			let mut title = wf.create_widget(Label::create)?;
			theme.primary_label(&mut title);
			title.set_text_static(cstr!("Borrow / Return"));
			title.set_width(percent(100));

			if packet.borrow_id.is_none() {
				let mut subtitle = wf.create_widget(Label::create)?;
				theme.secondary_label(&mut subtitle);
				subtitle.set_text_static(cstr!("Scan user"));
			}

			let mut item_container =
				wf.create_parent_widget(Obj::create, theme, |mut wf, theme| {
					let mut label = wf.create_widget(Label::create)?;
					label.set_text_static(cstr!("Item:"));
					label.set_align(Align::LeftMid, 0, 0);
					theme.misc_label(&mut label);

					let name_text =
						CString::new(packet.name.as_str()).unwrap_or(cstr!("?").to_owned());
					let mut name = wf.create_widget(Label::create)?;
					name.set_text(name_text.as_c_str());
					name.set_align(Align::RightMid, 0, 0);
					theme.misc_label(&mut name);

					Ok(())
				})?;
			theme.option_container(&mut item_container);
			item_container.set_width(percent(100));

			if packet.borrow_id.is_some() {
				let mut return_button =
					wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
						let mut label = wf.create_widget(Label::create)?;
						label.set_text_static(cstr!("Return"));
						Ok(())
					})?;
				theme.primary_button(&mut return_button);
				return_button.set_width(percent(100));

				let mut return_closure = |item_id: &str| {
					Unipage::try_new_with_http(
						Method::Post,
						"https://api.easy.drewbryan.org/return".to_string(),
						Some(ReturnRequest {
							item_id: item_id.to_string()
						}),
						vec![],
						theme,
						display,
						|packet: Packet<()>, mut wf, theme, display| {
							match packet {
								Packet::Ok(..) => {
									let mut label = wf.create_widget(Label::create)?;
									label.set_text_static(cstr!("Successfully\n returned"));
									label.set_width(percent(100));
									label.set_align(Align::Center, 0, 0);
									theme.primary_label(&mut label);

									let mut ok_button = wf.create_parent_widget(
										Btn::create,
										theme,
										|mut wf, _theme| {
											let mut label = wf.create_widget(Label::create)?;
											label.set_text_static(cstr!("Ok"));
											Ok(())
										}
									)?;
									theme.primary_button(&mut ok_button);
									ok_button.on_event(|_, e| {
										if let Event::Clicked = e {
											display.pop_page();
										}
									})?;
								}
								Packet::Error(code, err) => {
									err_page(code, err.message.as_str(), wf, theme, display)?;
								}
							}
							Ok(())
						},
						http_loading_screen
					)
					.unwrap();
				};

				return_button.on_event(move |_, e| {
					if let Event::Clicked = e {
						return_closure(packet.item_id.as_str())
					}
				})?;
			} else {
				Unipage::try_new_with_rfid_read(
					4..6,
					theme,
					display,
					move |data, mut wf, theme, display| {
						match data {
							Ok(data) => {
								if let Some(data_type) = data.get(0) {
									if *data_type == ITEM_TYPE {
										return err_page(
											0,
											"error: invalid card type (expected user, got item)",
											wf,
											theme,
											display
										);
									}

									let mut title = wf.create_widget(Label::create)?;
									title.set_text_static(cstr!("Please wait"));
									title.set_width(percent(100));
									title.set_align(Align::TopMid, 0, 0);
									theme.primary_label(&mut title);

									let login_token = u128::from_ne_bytes(
										data.get(1).cloned().unwrap_or([0u8; 16])
									);

									Unipage::try_new_with_http(
										Method::Post,
										"https://api.easy.drewbryan.org/login_using_perm_token"
											.to_string(),
										Some(LoginUsingPermanentTokenRequest {
											token: login_token
										}),
										vec![],
										theme,
										display,
										move |data, wf, theme, display| {
											confirm_borrow_page(packet, data, wf, theme, display)
										},
										http_loading_screen
									)?;
								}
							}
							Err(err) => {
								err_page(
									0,
									format!("error: {err:?}").as_str(),
									wf,
									theme,
									display
								)?;
							}
						}

						Ok(())
					}
				)?;
			}
		}
		Packet::Error(code, err) => {
			err_page(code, err.message.as_str(), wf, theme, display)?;
		}
	}

	Ok(())
}

fn confirm_borrow_page(
	item_info: GetItemInfoResponse,
	packet: Packet<LoginResponse>,
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	match packet {
		Packet::Ok(_, packet) => {
			let mut title = wf.create_widget(Label::create)?;
			title.set_text_static(cstr!("Please wait"));
			title.set_width(percent(100));
			title.set_align(Align::TopMid, 0, 0);
			theme.primary_label(&mut title);

			Unipage::try_new_with_http(
				Method::Get,
				"https://api.easy.drewbryan.org/logged_in_user".to_string(),
				Some(GetLoggedInUserRequest {
					session: Some(packet.session_id)
				}),
				vec![],
				theme,
				display,
				move |packet: Packet<GetUserInfoResponse>, mut wf, theme, display| {
					match packet {
						Packet::Ok(_, packet) => {
							let mut title = wf.create_widget(Label::create).unwrap();
							theme.primary_label(&mut title);
							title.set_text_static(cstr!("Borrow / Return"));
							title.set_width(percent(100));

							let mut item_container = wf
								.create_parent_widget(Obj::create, theme, |mut wf, theme| {
									let mut label = wf.create_widget(Label::create)?;
									label.set_text_static(cstr!("Item:"));
									label.set_align(Align::LeftMid, 0, 0);
									theme.misc_label(&mut label);

									let name_text = CString::new(item_info.name.as_str())
										.unwrap_or(cstr!("?").to_owned());
									let mut name = wf.create_widget(Label::create)?;
									name.set_text(name_text.as_c_str());
									name.set_align(Align::RightMid, 0, 0);
									theme.misc_label(&mut name);

									Ok(())
								})
								.unwrap();
							theme.option_container(&mut item_container);
							item_container.set_width(percent(100));

							let mut user_container = wf
								.create_parent_widget(Obj::create, theme, |mut wf, theme| {
									let mut label = wf.create_widget(Label::create)?;
									label.set_text_static(cstr!("User:"));
									label.set_align(Align::LeftMid, 0, 0);
									theme.misc_label(&mut label);

									let name_text = CString::new(packet.username.as_str())
										.unwrap_or(cstr!("?").to_owned());
									let mut name = wf.create_widget(Label::create)?;
									name.set_text(name_text.as_c_str());
									name.set_align(Align::RightMid, 0, 0);
									theme.misc_label(&mut name);

									Ok(())
								})
								.unwrap();
							theme.option_container(&mut user_container);
							user_container.set_width(percent(100));

							let mut borrow_button =
								wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
									let mut label = wf.create_widget(Label::create)?;
									label.set_text_static(cstr!("Borrow"));
									Ok(())
								})?;
							theme.primary_button(&mut borrow_button);
							borrow_button.set_width(percent(100));

							let mut borrow_closure = |item_id: &str, user_id: &str| {
								Unipage::try_new_with_http(
									Method::Post,
									"https://api.easy.drewbryan.org/borrow".to_string(),
									Some(BorrowRequest {
										item_id: item_id.to_string(),
										user_id: user_id.to_string()
									}),
									vec![],
									theme,
									display,
									|packet: Packet<()>, mut wf, theme, display| {
										match packet {
											Packet::Ok(..) => {
												let mut label = wf.create_widget(Label::create)?;
												label.set_text_static(cstr!(
													"Successfully\n borrowed"
												));
												label.set_align(Align::Center, 0, 0);
												label.set_width(percent(100));
												theme.primary_label(&mut label);

												let mut ok_button = wf.create_parent_widget(
													Btn::create,
													theme,
													|mut wf, _theme| {
														let mut label =
															wf.create_widget(Label::create)?;
														label.set_text_static(cstr!("Ok"));
														Ok(())
													}
												)?;
												theme.primary_button(&mut ok_button);
												ok_button.on_event(|_, e| {
													if let Event::Clicked = e {
														display.pop_page();
													}
												})?;
											}
											Packet::Error(code, err) => {
												err_page(
													code,
													err.message.as_str(),
													wf,
													theme,
													display
												)?;
											}
										}
										Ok(())
									},
									http_loading_screen
								)
								.unwrap();
							};

							borrow_button.on_event(move |_, e| {
								if let Event::Clicked = e {
									borrow_closure(
										item_info.item_id.as_str(),
										packet.user_id.as_str()
									)
								}
							})?;
						}
						Packet::Error(code, err) => {
							err_page(code, err.message.as_str(), wf, theme, display)?;
						}
					}

					Ok(())
				},
				http_loading_screen
			)?;
		}
		Packet::Error(code, err) => {
			err_page(code, err.message.as_str(), wf, theme, display)?;
		}
	}

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

	let mut option_list = wf.create_parent_widget(List::create, theme, |mut wf, theme| {
		let mut option_list_title = wf.create_widget(Label::create)?;
		option_list_title.set_text_static(cstr!("Options"));
		theme.secondary_label(&mut option_list_title);

		let mut new_card_option_container =
			wf.create_parent_widget(Obj::create, theme, |mut wf, theme| {
				let mut label = wf.create_widget(Label::create)?;
				label.set_text_static(cstr!("Card has been written before?"));
				theme.misc_label(&mut label);
				label.set_align(Align::LeftMid, 0, 0);

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
	theme.list(&mut option_list);
	option_list.set_width(percent(100));
	option_list.set_height(LV_SIZE_CONTENT);

	let mut final_buttons_list =
		wf.create_parent_widget(List::create, theme, |mut wf, theme| {
			let mut final_buttons_container =
				wf.create_parent_widget(Obj::create, theme, |mut wf, theme| {
					let mut item_button =
						wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
							let mut label = wf.create_widget(Label::create)?;
							label.set_text_static(cstr!("Item"));
							Ok(())
						})?;
					theme.primary_button(&mut item_button);
					item_button.set_align(Align::LeftMid, 0, 0);
					item_button.set_width(percent(45));

					let mut program_item_closure = |default: bool| {
						if let Err(err) = Unipage::try_new_with_http(
							Method::Get,
							"https://api.easy.drewbryan.org/items".to_string(),
							None::<()>,
							vec![],
							theme,
							display,
							move |packet, wf, theme, display| {
								program_item_page(packet, default, wf, theme, display)
							},
							http_loading_screen
						) {
							error!("Failed to make page with http request ({err})");
							display.pop_page();
						}
					};

					let item_button_default_flag = write_new_card_option.clone();
					item_button.on_event(move |_, e| {
						if let Event::Clicked = e {
							program_item_closure(!item_button_default_flag.get())
						}
					})?;

					let mut user_button =
						wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
							let mut label = wf.create_widget(Label::create)?;
							label.set_text_static(cstr!("User"));
							Ok(())
						})?;
					theme.primary_button(&mut user_button);
					user_button.set_align(Align::RightMid, 0, 0);
					user_button.set_width(percent(45));

					let mut program_user_closure = |default: bool| {
						if let Err(err) = Unipage::try_new_with_http(
							Method::Get,
							"https://api.easy.drewbryan.org/users".to_string(),
							None::<()>,
							vec![],
							theme,
							display,
							move |packet, wf, theme, display| {
								program_user_page(packet, default, wf, theme, display)
							},
							http_loading_screen
						) {
							error!("Failed to make page with http request ({err})");
							// todo move this inside the request
							display.pop_page();
						}
					};

					user_button.on_event(move |_, e| {
						if let Event::Clicked = e {
							program_user_closure(!write_new_card_option.get())
						}
					})?;

					Ok(())
				})?;
			theme.option_container(&mut final_buttons_container);
			final_buttons_container.set_width(percent(100));

			let mut cancel_button =
				wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
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
		})?;
	theme.list(&mut final_buttons_list);
	final_buttons_list.set_width(percent(100));
	final_buttons_list.set_height(LV_SIZE_CONTENT);

	Ok(())
}

fn program_item_page(
	packet: Packet<GetItemsResponse>,
	default: bool,
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	match packet {
		Packet::Ok(_, packet) => {
			let mut title = wf.create_widget(Label::create)?;
			title.set_text_static(cstr!("Select item"));
			theme.primary_label(&mut title);

			let mut list = wf.create_parent_widget(List::create, theme, |mut wf, theme| {
				for item in packet.items {
					let mut failed_name = false;
					let mut item_button =
						wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
							let mut label = wf.create_widget(Label::create)?;
							let name = match CString::new(item.name) {
								Ok(name) => name,
								Err(err) => {
									failed_name = true;
									CString::new(format!("Unknown ({err})")).unwrap()
								}
							};
							label.set_text(name.as_c_str());
							Ok(())
						})?;
					item_button.set_width(percent(100));
					theme.misc_button(&mut item_button);
					if failed_name {
						unsafe {
							lv_obj_add_state(
								item_button.raw().as_ptr(),
								LV_STATE_DISABLED as lv_state_t
							);
						}
					}

					let mut write_item_on_card = |item_id: u128, default: bool| {
						let page = match Unipage::try_new(theme, display, |wf, theme, display| {
							write_item_page(item_id, default, wf, theme, display)
						}) {
							Ok(page) => page,
							Err(err) => {
								error!("Failed to make page ({err})");
								return;
							}
						};
						display.push_page(page);
					};

					match Uuid::try_parse(item.item_id.as_str()) {
						Ok(uuid) => {
							item_button.on_event(move |_, e| {
								if let Event::Clicked = e {
									write_item_on_card(uuid.as_u128(), default);
								}
							})?;
						}
						Err(err) => {
							error!("Invalid item id ({err:?})");
							unsafe {
								lv_obj_add_state(
									item_button.raw().as_ptr(),
									LV_STATE_DISABLED as lv_state_t
								);
							}
						}
					}
				}
				Ok(())
			})?;
			theme.list(&mut list);
			list.set_width(percent(100));
		}
		Packet::Error(code, err) => {
			err_page(code, err.message.as_str(), wf, theme, display)?;
		}
	}

	Ok(())
}

fn write_item_page(
	item_id: u128,
	default: bool,
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	let mut title = wf.create_widget(Label::create)?;
	title.set_text_static(cstr!("Scan card"));
	theme.primary_label(&mut title);

	Unipage::try_new_with_rfid_write(
		vec![(4, ITEM_TYPE), (5, item_id.to_ne_bytes())],
		default,
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

			let mut ok_button = wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
				let mut label = wf.create_widget(Label::create)?;
				label.set_text_static(cstr!("Ok"));
				Ok(())
			})?;
			theme.primary_button(&mut ok_button);
			ok_button.on_event(|_, e| {
				if let Event::Clicked = e {
					display.pop_page();
				}
			})?;

			Ok(())
		}
	)?;

	Ok(())
}

fn program_user_page(
	packet: Packet<GetUsersResponse>,
	default: bool,
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	match packet {
		Packet::Ok(_, packet) => {
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

					let mut obtain_token = |user_id: &str, default: bool| {
						if let Err(err) = Unipage::try_new_with_http(
							Method::Post,
							"https://api.easy.drewbryan.org/obtain_permanent_token".to_string(),
							Some(ObtainPermanentTokenRequest {
								user_id: user_id.to_string()
							}),
							vec![],
							theme,
							display,
							move |packet, wf, theme, display| {
								program_obtained_permanent_token(
									packet, default, wf, theme, display
								)
							},
							http_loading_screen
						) {
							error!("Failed to push page with obtained token ({err})");
							display.pop_page();
						}
					};

					let user_id = user.user_id;
					user_button.on_event(move |_, e| {
						if let Event::Clicked = e {
							obtain_token(user_id.as_str(), default);
						}
					})?;
				}
				Ok(())
			})?;
			theme.list(&mut list);
			list.set_width(percent(100));
		}
		Packet::Error(code, err) => {
			err_page(code, err.message.as_str(), wf, theme, display)?;
		}
	}
	Ok(())
}

fn program_obtained_permanent_token(
	packet: Packet<ObtainPermanentTokenResponse>,
	default: bool,
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	match packet {
		Packet::Ok(_, packet) => {
			let mut title = wf.create_widget(Label::create)?;
			title.set_text_static(cstr!("Scan card"));
			theme.primary_label(&mut title);

			Unipage::try_new_with_rfid_write(
				vec![(4, USER_TYPE), (5, packet.token.to_ne_bytes())],
				default,
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
							display.pop_page();
						}
					})?;

					Ok(())
				}
			)?;
		}
		Packet::Error(code, err) => {
			err_page(code, err.message.as_str(), wf, theme, display)?;
		}
	}

	Ok(())
}
