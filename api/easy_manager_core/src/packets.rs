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

#[tracing::instrument]
pub fn validate_packet(headers: &axum::http::HeaderMap) -> Result<(), PacketError> {
	let client_version: &axum::http::HeaderValue = headers
		.get(CLIENT_VERSION)
		.ok_or(PacketError::MissingHeaderError(CLIENT_VERSION))
		.inspect_err(|err: &PacketError| tracing::error!(%err, "Invalid packet"))?;

	if client_version != env!("CARGO_PKG_VERSION") {
		return Err(PacketError::InvalidVersion(client_version.clone()));
	}

	Ok(())
}

pub mod get {

	#[derive(serde::Serialize)]
	pub struct DeviceInfo {}
}
