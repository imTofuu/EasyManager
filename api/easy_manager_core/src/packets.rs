use alloc::string::String;
use core::error::Error;
use core::fmt::{Display, Formatter};

use serde::Deserialize;

pub const CLIENT_VERSION_HN: &str = "client-version";

#[derive(Debug, Clone)]
pub enum PacketError {
	MissingHeader(String),
	InvalidVersion(String)
}

impl Display for PacketError {
	fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), core::fmt::Error> {
		match self {
			PacketError::MissingHeader(header_name) => write!(f, "{header_name}"),
			PacketError::InvalidVersion(version) => write!(f, "{version:?}")
		}
	}
}

impl Error for PacketError {}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ErrorPacket {
	pub message: String
}

#[derive(Deserialize)]
pub enum Packet<T: serde::Serialize> {
	Ok(T),
	Error(ErrorPacket)
}

pub mod get {
	use alloc::boxed::Box;
	use alloc::string::String;

	use crate::PermissionLevel;

	#[derive(Debug, serde::Serialize, serde::Deserialize)]
	pub struct GetUserInfoResponse {
		pub username:         String,
		pub permission_level: PermissionLevel
	}

	#[derive(serde::Serialize, serde::Deserialize)]
	pub struct GetUsersResponse {
		pub users: Box<[GetUserInfoResponse]>
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
		pub password:           String
	}

	#[derive(serde::Serialize, serde::Deserialize)]
	pub struct LoginResponse {
		pub session_id: String
	}

	#[derive(serde::Serialize, serde::Deserialize)]
	pub struct CreateUserRequest {
		pub email:            String,
		pub username:         String,
		pub password:         String,
		pub permission_level: PermissionLevel
	}

	#[derive(Debug, serde::Serialize, serde::Deserialize)]
	pub struct CreateItemModelRequest {
		pub name:             String,
		pub description:      Option<String>,
		pub permission_level: PermissionLevel
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
