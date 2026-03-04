use axum::response::Response;
use std::error::Error;
use std::fmt::{Display, Formatter};

pub const CLIENT_VERSION: axum::http::HeaderName =
	axum::http::HeaderName::from_static("client-version");

pub fn get_default_client_headers() -> axum::http::HeaderMap {
	let mut result = axum::http::HeaderMap::new();

	result.insert(CLIENT_VERSION, env!("CARGO_PKG_VERSION").parse().unwrap());

	result
}

#[derive(Debug, Clone)]
pub enum PacketError {
	MissingHeader(axum::http::HeaderName),
	InvalidVersion(axum::http::HeaderValue),
}

impl Display for PacketError {
	fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			PacketError::MissingHeader(header_name) => write!(f, "{header_name}"),
			PacketError::InvalidVersion(version) => write!(f, "{version:?}"),
		}
	}
}

impl Error for PacketError {}

#[tracing::instrument]
pub fn validate_packet(headers: &axum::http::HeaderMap) -> Result<(), PacketError> {
	let client_version: &axum::http::HeaderValue = headers
		.get(CLIENT_VERSION)
		.ok_or(PacketError::MissingHeader(CLIENT_VERSION))?;

	if client_version != env!("CARGO_PKG_VERSION") {
		Err(PacketError::InvalidVersion(client_version.to_owned()))
	} else {
		Ok(())
	}
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ErrorPacket {
	pub message: String,
}

pub enum Packet<T: serde::Serialize + serde::de::DeserializeOwned> {
	Ok(T),
	Error(ErrorPacket),
}

impl<T: serde::Serialize + serde::de::DeserializeOwned> axum::response::IntoResponse for Packet<T> {
	fn into_response(self) -> Response {
		match self {
			Packet::Ok(val) => axum::Json(val).into_response(),
			Packet::Error(error_packet) => axum::Json(error_packet).into_response(),
		}
	}
}

pub mod get {}

pub mod post {
	use std::fmt::{Debug, Formatter};
	
	#[derive(Debug, serde::Serialize, serde::Deserialize)]
	pub struct GenerateSessionResponse {
		pub session_id: String,
	}
	
	#[derive(serde::Serialize, serde::Deserialize)]
	pub struct CreateUserRequest {
		pub username: String,
		pub password_hash: String,
	}
	
	#[derive(Debug, serde::Serialize, serde::Deserialize)]
	pub struct CreateUserResponse {
		pub user_id: String,
	}
	
	impl Debug for CreateUserRequest {
		fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
			f.debug_struct("CreateUserRequest")
				.field("username", &self.username)
				.field("password_hash", &"password_hash".to_owned())
				.finish()
		}
	}
}
