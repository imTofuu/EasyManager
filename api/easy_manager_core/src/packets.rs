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

#[derive(Debug)]
pub enum PacketError {
	MissingHeaderError(axum::http::HeaderName),
	InvalidVersion(axum::http::HeaderValue),
}

impl Display for PacketError {
	fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			PacketError::MissingHeaderError(header_name) => write!(f, "{header_name}"),
			PacketError::InvalidVersion(version) => write!(f, "{version:?}"),
		}
	}
}

impl Error for PacketError {}

pub enum PacketState {
	Auth(String),
	Unauth,
	Invalid(PacketError),
}

#[tracing::instrument]
pub fn parse_packet_state(headers: &axum::http::HeaderMap) -> PacketState {
	let client_version: &axum::http::HeaderValue = match headers
		.get(CLIENT_VERSION)
		.ok_or(PacketError::MissingHeaderError(CLIENT_VERSION))
	{
		Ok(headers) => headers,
		Err(err) => return PacketState::Invalid(err),
	};

	if client_version != env!("CARGO_PKG_VERSION") {
		return PacketState::Invalid(PacketError::InvalidVersion(client_version.clone()));
	}

	match headers
		.get(axum::http::header::AUTHORIZATION)
		.ok_or(PacketError::MissingHeaderError(
			axum::http::header::AUTHORIZATION,
		)) {
		Ok(id) => id
			.to_str()
			.map(|id_str| PacketState::Auth(id_str.to_owned()))
			.unwrap_or(PacketState::Unauth),
		Err(err) => PacketState::Invalid(err),
	}
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ErrorPacket {
	message: String,
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

	#[derive(serde::Serialize, serde::Deserialize)]
	pub struct GenerateSession {
		pub session_id: String,
	}
	
	#[derive(serde::Serialize, serde::Deserialize)]
	pub struct CreateUserResponse {
		pub user_id: String
	}
	
	#[derive(serde::Serialize, serde::Deserialize)]
	pub struct CreateUserRequest {
		pub username: String,
		pub password_hash: String
	}
}
