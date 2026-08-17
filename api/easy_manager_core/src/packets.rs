use alloc::string::String;
use core::error::Error;
use core::fmt::{Display, Formatter};

use serde::Deserialize;

pub const CLIENT_VERSION_HN: &str = "client-version";

#[derive(Debug, Clone)]
pub enum PacketError {
	MissingHeader(String),
	InvalidVersion(String),
}

impl Display for PacketError {
	fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), core::fmt::Error> {
		match self {
			PacketError::MissingHeader(header_name) => write!(f, "{header_name}"),
			PacketError::InvalidVersion(version) => write!(f, "{version:?}"),
		}
	}
}

impl Error for PacketError {}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ErrorPacket {
	pub message: String,
}

#[derive(serde::Serialize, Deserialize)]
pub enum Packet<T> {
	Ok(u16, T),
	Error(u16, ErrorPacket),
}

pub mod get {
	use alloc::boxed::Box;
	use alloc::string::String;

	use crate::PermissionLevel;

	#[derive(Debug, serde::Serialize, serde::Deserialize)]
	pub struct GetUserInfoResponse {
		pub username: String,
		pub user_id: String,
		pub permission_level: PermissionLevel,
	}

	#[derive(serde::Serialize, serde::Deserialize)]
	pub struct GetItemInfoResponse {
		pub name: String,
		pub item_id: String,
		pub permission_level: PermissionLevel,
		pub item_model_id: String,
		pub borrow_id: Option<String>,
	}

	#[derive(serde::Serialize, serde::Deserialize)]
	pub struct GetUsersResponse {
		pub users: Box<[GetUserInfoResponse]>,
	}

	#[derive(serde::Serialize, serde::Deserialize)]
	pub struct GetItemsResponse {
		pub items: Box<[GetItemInfoResponse]>,
	}
}

pub mod post {
	use alloc::borrow::ToOwned;
	use alloc::string::String;
	use core::fmt::{Debug, Formatter};

	use crate::{AccountIdentifier, PermissionLevel};

	#[derive(serde::Serialize, serde::Deserialize)]
	pub struct LoginRequest {
		pub account_identifier: AccountIdentifier,
		pub password: String,
	}

	#[derive(serde::Serialize, serde::Deserialize)]
	pub struct LoginResponse {
		pub session_id: String,
	}

	#[derive(serde::Serialize, serde::Deserialize)]
	pub struct CreateUserRequest {
		pub email: String,
		pub username: String,
		pub password: String,
		pub permission_level: PermissionLevel,
	}

	#[derive(Debug, serde::Serialize, serde::Deserialize)]
	pub struct CreateItemModelRequest {
		pub name: String,
		pub description: Option<String>,
		pub permission_level: PermissionLevel,
	}

	#[derive(Debug, serde::Serialize, serde::Deserialize)]
	pub struct ObtainPermanentTokenRequest {
		pub user_id: Option<String>,
	}

	#[derive(serde::Serialize, serde::Deserialize)]
	pub struct ObtainPermanentTokenResponse {
		pub token: u128,
	}

	#[derive(Debug, serde::Serialize, serde::Deserialize)]
	pub struct LoginUsingPermanentTokenRequest {
		pub token: u128,
	}

	#[derive(Debug, serde::Serialize, serde::Deserialize)]
	pub struct BorrowRequest {
		pub item_id: String,
	}

	#[derive(Debug, serde::Serialize, serde::Deserialize)]
	pub struct ReturnRequest {
		pub item_id: String,
	}

	impl Debug for LoginRequest {
		fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), core::fmt::Error> {
			f.debug_struct("LoginRequest")
				.field("account_identifier", &self.account_identifier)
				.field("password", &"password".to_owned())
				.finish()
		}
	}

	impl Debug for CreateUserRequest {
		fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), core::fmt::Error> {
			f.debug_struct("CreateUserRequest")
				.field("username", &self.username)
				.field("password", &"password".to_owned())
				.finish()
		}
	}
}
