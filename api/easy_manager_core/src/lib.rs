#![no_std]
extern crate alloc;

use alloc::borrow::ToOwned;
use alloc::string::String;

pub mod packets;

pub fn get_core_version() -> &'static str { env!("CARGO_PKG_VERSION") }

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum AccountIdentifier {
	Email { email: String },
	Username { username: String }
}

#[derive(Debug, Eq, PartialEq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionLevel {
	Default,
	Admin
}

impl From<PermissionLevel> for String {
	fn from(value: PermissionLevel) -> Self {
		match value {
			PermissionLevel::Admin => "admin".to_owned(),
			PermissionLevel::Default => "default".to_owned()
		}
	}
}

impl TryFrom<String> for PermissionLevel {
	type Error = String;

	fn try_from(value: String) -> Result<Self, Self::Error> {
		match value.as_str() {
			"admin" => Ok(Self::Admin),
			"default" => Ok(Self::Default),
			_ => Err("invalid permission level".to_owned())
		}
	}
}
