use std::cell::Cell;
use std::rc::Rc;

use cstr_core::{CString, cstr};
use easy_manager_core::PermissionLevel;
use easy_manager_core::packets::Packet;
use easy_manager_core::packets::get::{
	GetItemInfoResponse,
	GetItemsResponse,
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
use log::error;
use lvgl::misc::area::LV_SIZE_CONTENT;
use lvgl::sys::{
	LV_OBJ_FLAG_CLICKABLE,
	LV_STATE_CHECKED,
	LV_STATE_DISABLED,
	lv_obj_add_flag,
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

pub const fn percent(percent: u32) -> u32 { (1 << 13) | percent }

fn http_loading_screen(
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	_display: &mut InteractableDisplay
) -> LvResult<()> {
	let mut label = wf.create_widget(Label::create)?;
	theme.primary_label(&mut label);
	label.set_text_static(cstr!("Waiting for server..."));
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
			.unwrap_or(CString::new("Failed to parse error code").unwrap())
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

pub fn home_page(
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	let mut title = wf.create_widget(Label::create)?;
	title.set_text_static(cstr!("Easy Manager"));
	title.set_width(percent(100));
	title.set_align(Align::Center, 0, 0);
	theme.primary_label(&mut title);

	let mut list = wf.create_parent_widget(List::create, theme, |mut wf, theme| {
		let mut login_button = wf.create_parent_widget(Btn::create, theme, |mut wf, theme| {
			let mut label = wf.create_widget(Label::create)?;
			label.set_text_static(cstr!("Login"));
			Ok(())
		})?;
		theme.primary_button(&mut login_button);
		login_button.set_width(percent(100));
		login_button.on_event(|_, e| {
			if let Event::Clicked = e {
				match Unipage::try_new(theme, display, login_page) {
					Ok(page) => display.push_page(page),
					Err(err) => error!("Something went wrong creating the borrowing page ({err})")
				}
			}
		})?;

		let mut view_inventory_button =
			wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
				let mut label = wf.create_widget(Label::create)?;
				label.set_text_static(cstr!("View Inventory"));
				label.set_width(percent(100));
				Ok(())
			})?;
		theme.secondary_button(&mut view_inventory_button);
		view_inventory_button.set_width(percent(100));
		view_inventory_button.on_event(|_, e| {
			if let Event::Clicked = e {
				if let Err(err) = Unipage::try_new_with_http(
					Method::Get,
					"https://api.easy.drewbryan.org/items".to_string(),
					None::<()>,
					vec![],
					theme,
					display,
					view_inventory_page,
					http_loading_screen
				) {
					match Unipage::try_new(theme, display, |wf, theme, display| {
						err_page(
							0,
							format!("Failed to create inventory viewer page ({err:?})").as_str(),
							wf,
							theme,
							display
						)
					}) {
						Ok(page) => display.push_page(page),
						Err(err) => {
							error!("Failed to create error page ({err:?})");
						}
					}
				}
			}
		})?;

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
	theme.list(&mut list);
	list.set_width(percent(100));

	Ok(())
}

pub fn login_page(
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	let mut title = wf.create_widget(Label::create)?;
	title.set_text_static(cstr!("Login"));
	title.set_width(percent(100));
	title.set_align(Align::Center, 0, 0);
	theme.primary_label(&mut title);

	let mut subtitle = wf.create_widget(Label::create)?;
	subtitle.set_text_static(cstr!("Please scan card"));
	subtitle.set_width(percent(100));
	subtitle.set_align(Align::Center, 0, 0);
	theme.secondary_label(&mut subtitle);

	Unipage::try_new_with_rfid_read(4..6, theme, display, |data, mut wf, theme, display| {
		match data {
			Ok(data) => {
				match data.get(0) {
					Some(data_type) => {
						if *data_type != USER_TYPE {
							return err_page(0, "error: invalid card type", wf, theme, display);
						}
						match data.get(1).map(|&bytes| u128::from_ne_bytes(bytes)) {
							Some(token) => {
								let mut label = wf.create_widget(Label::create)?;
								theme.primary_label(&mut label);
								label.set_text_static(cstr!("Waiting for server..."));
								label.set_width(percent(100));
								label.set_align(Align::TopMid, 0, 0);
								Unipage::try_new_with_http(
									Method::Post,
									"https://api.easy.drewbryan.org/login_using_perm_token"
										.to_string(),
									Some(LoginUsingPermanentTokenRequest { token }),
									vec![],
									theme,
									display,
									|packet: Packet<LoginResponse>, mut wf, theme, display| {
										match packet {
											Packet::Ok(..) => {
												let mut label = wf.create_widget(Label::create)?;
												theme.primary_label(&mut label);
												label.set_text_static(cstr!(
													"Waiting for server..."
												));
												label.set_width(percent(100));
												label.set_align(Align::TopMid, 0, 0);
												Unipage::try_new_with_http(
													Method::Get,
													"https://api.easy.drewbryan.org/logged_in_user"
														.to_string(),
													None::<()>,
													vec![],
													theme,
													display,
													main_page,
													http_loading_screen
												)?;
											}
											Packet::Error(code, err) => {
												return err_page(
													code,
													err.message.as_str(),
													wf,
													theme,
													display
												);
											}
											Packet::None(code) => {
												return err_page(
													code,
													"unexpected empty packet",
													wf,
													theme,
													display
												);
											}
										}
										Ok(())
									},
									http_loading_screen
								)?;
							}
							None => {
								return err_page(
									0,
									"error: missing user token",
									wf,
									theme,
									display
								);
							}
						}
					}
					None => {
						return err_page(0, "error: missing data type in card", wf, theme, display);
					}
				}
			}
			Err(err) => {
				return err_page(0, format!("error: {err:?}").as_str(), wf, theme, display);
			}
		}
		Ok(())
	})?;

	Ok(())
}

pub fn main_page(
	user: Packet<GetUserInfoResponse>,
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	match user {
		Packet::Ok(_, packet) => {
			let mut title = wf.create_widget(Label::create)?;
			theme.primary_label(&mut title);
			title.set_text_static(cstr!("EasyManager"));
			title.set_width(percent(100));

			let mut subtitle = wf.create_widget(Label::create)?;
			theme.secondary_label(&mut subtitle);
			subtitle.set_text_static(cstr!("Scan item or use menu"));
			subtitle.set_width(percent(100));

			let mut list = wf.create_parent_widget(List::create, theme, |mut wf, theme| {
				let mut borrow_button =
					wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
						let mut label = wf.create_widget(Label::create)?;
						label.set_text_static(cstr!("Borrow / Return"));
						label.set_width(percent(100));
						Ok(())
					})?;
				borrow_button.set_width(percent(100));
				theme.primary_button(&mut borrow_button);
				borrow_button.on_event(|_, e| {
					if let Event::Clicked = e {
						match Unipage::try_new(theme, display, borrow_page) {
							Ok(page) => display.push_page(page),
							Err(err) => {
								error!("Something went wrong creating the borrowing page ({err})")
							}
						}
					}
				})?;

				if packet.permission_level >= PermissionLevel::Admin {
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
								Err(err) => {
									error!(
										"Something went wrong creating the programming page \
										 ({err})"
									)
								}
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
				}

				let mut logout_button =
					wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
						let mut label = wf.create_widget(Label::create)?;
						label.set_text_static(cstr!("Logout"));
						label.set_width(percent(100));
						Ok(())
					})?;
				logout_button.set_width(percent(100));
				theme.misc_button(&mut logout_button);
				logout_button.on_event(|_, e| {
					if let Event::Clicked = e {
						Unipage::try_new_with_http(
							Method::Post,
							"https://api.easy.drewbryan.org/logout".to_string(),
							None::<()>,
							vec![],
							theme,
							display,
							|packet: Packet<()>, mut wf, theme, display| {
								match packet {
									Packet::Ok(..) | Packet::None(_) => {
										let mut label = wf.create_widget(Label::create)?;
										label.set_text_static(cstr!("Successfully\n logged out"));
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
										let mut title = wf.create_widget(Label::create)?;
										title.set_text_static(cstr!("Error:"));
										title.set_width(percent(100));
										theme.primary_label(&mut title);

										let mut code_label = wf.create_widget(Label::create)?;
										code_label.set_text(
											CString::new(code.to_string().as_str())
												.unwrap_or(
													CString::new("Failed to parse error code")
														.unwrap()
												)
												.as_c_str()
										);
										code_label.set_width(percent(100));
										theme.secondary_label(&mut code_label);

										let mut message = wf.create_widget(Label::create)?;
										message.set_text_static(
											CString::new(err.message.as_str())
												.unwrap_or(cstr!("Unknown error").to_owned())
												.as_c_str()
										);
										message.set_width(percent(100));
										theme.misc_label(&mut message);

										let mut reboot_button = wf.create_parent_widget(
											Btn::create,
											theme,
											|mut wf, _theme| {
												let mut label = wf.create_widget(Label::create)?;
												label.set_text_static(cstr!("Reboot"));
												Ok(())
											}
										)?;
										reboot_button.set_width(percent(100));
										theme.primary_button(&mut reboot_button);
										reboot_button.on_event(|_, e| {
											if let Event::Clicked = e {
												unsafe { esp_restart() };
											}
										})?;
									}
								};

								Ok(())
							},
							http_loading_screen
						)
						.unwrap();
					}
				})?;

				Ok(())
			})?;
			list.set_width(percent(100));
			theme.list(&mut list);
		}
		Packet::Error(code, err) => {
			return err_page(code, err.message.as_str(), wf, theme, display);
		}
		Packet::None(code) => {
			return err_page(code, "unexpected empty packet", wf, theme, display);
		}
	}

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
					if *data_type != ITEM_TYPE {
						return err_page(
							0,
							"error: invalid card type (expected item",
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
						confirm_borrow,
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

fn confirm_borrow(
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
								Packet::None(code) => {
									return err_page(
										code,
										"unexpected empty packet",
										wf,
										theme,
										display
									);
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
				let mut borrow_button =
					wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
						let mut label = wf.create_widget(Label::create)?;
						label.set_text_static(cstr!("Borrow"));
						Ok(())
					})?;
				theme.primary_button(&mut borrow_button);
				borrow_button.set_width(percent(100));

				let mut borrow_closure = |item_id: &str| {
					Unipage::try_new_with_http(
						Method::Post,
						"https://api.easy.drewbryan.org/borrow".to_string(),
						Some(BorrowRequest {
							item_id: item_id.to_string()
						}),
						vec![],
						theme,
						display,
						|packet: Packet<()>, mut wf, theme, display| {
							match packet {
								Packet::Ok(..) => {
									let mut label = wf.create_widget(Label::create)?;
									label.set_text_static(cstr!("Successfully\n borrowed"));
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
								Packet::None(code) => {
									return err_page(
										code,
										"unexpected empty packet",
										wf,
										theme,
										display
									);
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
						borrow_closure(packet.item_id.as_str())
					}
				})?;
			}
		}
		Packet::Error(code, err) => {
			err_page(code, err.message.as_str(), wf, theme, display)?;
		}
		Packet::None(code) => {
			return err_page(code, "unexpected empty packet", wf, theme, display);
		}
	}

	Ok(())
}

fn view_inventory_page(
	packet: Packet<GetItemsResponse>,
	mut wf: WidgetFactory<Screen>,
	theme: &'static impl Theme,
	display: &mut InteractableDisplay
) -> LvResult<()> {
	match packet {
		Packet::Ok(_, packet) => {
			let mut title = wf.create_widget(Label::create)?;
			title.set_text_static(cstr!("Inventory"));
			title.set_width(percent(100));
			theme.primary_label(&mut title);

			let mut list = wf.create_parent_widget(List::create, theme, |mut wf, theme| {
				for item in packet.items {
					let mut container =
						wf.create_parent_widget(Obj::create, theme, |mut wf, theme| {
							let mut name = wf.create_widget(Label::create)?;
							name.set_text(
								CString::new(item.name)
									.unwrap_or(cstr!("?").to_owned())
									.as_c_str()
							);
							name.set_align(Align::LeftMid, 0, 0);
							unsafe {
								lv_obj_add_flag(name.raw().as_ptr(), LV_OBJ_FLAG_CLICKABLE);
							}

							let mut state = wf.create_widget(Label::create)?;
							state.set_text_static(match item.borrow_id {
								Some(_) => cstr!("Borrowed"),
								None => cstr!("Available")
							});
							state.set_align(Align::RightMid, 0, 0);
							Ok(())
						})?;
					theme.option_container(&mut container);
					container.set_width(percent(100));
				}

				Ok(())
			})?;
			theme.list(&mut list);
			list.set_width(percent(100));

			let mut back_button =
				wf.create_parent_widget(Btn::create, theme, |mut wf, _theme| {
					let mut label = wf.create_widget(Label::create)?;
					label.set_text_static(cstr!("Back"));
					label.set_width(percent(100));
					Ok(())
				})?;
			theme.primary_button(&mut back_button);
			back_button.set_width(percent(100));
			back_button.on_event(|_, e| {
				if let Event::Clicked = e {
					display.pop_page();
				}
			})?;
		}
		Packet::Error(code, err) => {
			err_page(code, err.message.as_str(), wf, theme, display)?;
		}
		Packet::None(code) => {
			return err_page(code, "unexpected empty packet", wf, theme, display);
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
							move |packet: Packet<GetUsersResponse>, wf, theme, display| {
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
						display.pop_page();
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
		Packet::None(code) => {
			return err_page(code, "unexpected empty packet", wf, theme, display);
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
								user_id: Some(user_id.to_string())
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
		Packet::None(code) => {
			return err_page(code, "unexpected empty packet", wf, theme, display);
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
		Packet::None(code) => {
			return err_page(code, "unexpected empty packet", wf, theme, display);
		}
	}

	Ok(())
}
