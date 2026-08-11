#![warn(clippy::unwrap_used)]

mod endpoints;
mod entity;

use std::io;
use std::str::FromStr;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, Response, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::routing::method_routing;
use axum::{Router, middleware};
use axum_extra::extract::CookieJar;
use easy_manager_core::PermissionLevel;
use easy_manager_core::packets::post::CreateUserRequest;
use easy_manager_core::packets::{CLIENT_VERSION_HN, ErrorPacket, Packet, PacketError};
use migration::MigratorTrait;
use sea_orm::{DatabaseConnection, EntityTrait};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use uuid::Uuid;

use crate::endpoints::create_user_unchecked;
use crate::entity::prelude::Session;

#[tracing::instrument]
fn init_logging() -> tracing_appender::non_blocking::WorkerGuard {
	let mut exe_path: std::path::PathBuf =
		std::env::current_exe().expect("Could not find executable location");
	exe_path.pop();

	let (file_writer, file_guard) = tracing_appender::non_blocking(
		tracing_appender::rolling::daily(exe_path.join("logs"), "easy_manager_server"),
	);

	let file_layer = tracing_subscriber::fmt::layer()
		.compact()
		.with_ansi(false)
		.with_writer(file_writer)
		.with_filter(tracing_subscriber::filter::LevelFilter::from_level(
			if cfg!(debug_assertions) {
				tracing::Level::DEBUG
			} else {
				tracing::Level::INFO
			},
		));

	let cout_layer = tracing_subscriber::fmt::layer()
		.pretty()
		.with_ansi(true)
		.with_filter(tracing_subscriber::filter::Targets::new().with_default(
			tracing_subscriber::filter::LevelFilter::from_level(if cfg!(debug_assertions) {
				tracing::Level::DEBUG
			} else {
				tracing::Level::INFO
			}),
		));

	tracing_subscriber::Registry::default()
		.with(file_layer)
		.with(cout_layer)
		.init();

	tracing::info!(log_dir = ?exe_path, "Logging initialised");

	file_guard
}

struct ResponsePacket<T: serde::Serialize + serde::de::DeserializeOwned>(Packet<T>);

impl<T: serde::Serialize + serde::de::DeserializeOwned> IntoResponse for ResponsePacket<T> {
	fn into_response(self) -> axum::response::Response {
		match self.0 {
			Packet::Ok(val) => axum::Json(val).into_response(),
			Packet::Error(error_packet) => axum::Json(error_packet).into_response(),
		}
	}
}

impl<T: serde::Serialize + serde::de::DeserializeOwned> From<Packet<T>> for ResponsePacket<T> {
	fn from(value: Packet<T>) -> Self {
		Self(value)
	}
}

#[tracing::instrument]
async fn auth_middleware(
	state: State<DatabaseConnection>,
	cookie_jar: CookieJar,
	mut req: Request<Body>,
	next: Next,
) -> Response<Body> {
	let session_id: Uuid = match cookie_jar.get("session") {
		Some(session_id) => match Uuid::from_str(session_id.value()) {
			Ok(session_id) => session_id,
			Err(_) => {
				let (_, cookie_jar) = endpoints::logout(state, cookie_jar).await;
				return (
					StatusCode::UNAUTHORIZED,
					cookie_jar,
					ResponsePacket::from(Packet::<()>::Error(ErrorPacket {
						message: "Invalid session, please login again".to_owned(),
					})),
				)
					.into_response();
			}
		},
		None => {
			return (
				StatusCode::UNAUTHORIZED,
				cookie_jar,
				ResponsePacket::from(Packet::<()>::Error(ErrorPacket {
					message: "Please login".to_owned(),
				})),
			)
				.into_response();
		}
	};

	let session = match Session::find_by_id(session_id).one(&state.0).await {
		Ok(session) => match session {
			Some(session) => {
				if session.expired {
					let (_, cookie_jar) = endpoints::logout(state, cookie_jar).await;
					return (
						StatusCode::FORBIDDEN,
						cookie_jar,
						ResponsePacket::from(Packet::<()>::Error(ErrorPacket {
							message: "Session is expired, please login again".to_owned(),
						})),
					)
						.into_response();
				}
				session
			}
			None => {
				let (_, cookie_jar) = endpoints::logout(state, cookie_jar).await;
				return (
					StatusCode::FORBIDDEN,
					cookie_jar,
					ResponsePacket::from(Packet::<()>::Error(ErrorPacket {
						message: "Session is not found, please login again".to_owned(),
					})),
				)
					.into_response();
			}
		},
		Err(err) => {
			tracing::error!(%err, "An error occurred validating a session");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				cookie_jar,
				ResponsePacket::from(Packet::<()>::Error(ErrorPacket {
					message: "Something went wrong validating the session".to_owned(),
				})),
			)
				.into_response();
		}
	};
	req.extensions_mut().insert(session);
	next.run(req).await
}

#[tracing::instrument]
pub fn validate_packet(headers: &axum::http::HeaderMap) -> Result<(), PacketError> {
	let client_version: &axum::http::HeaderValue = headers
		.get(CLIENT_VERSION_HN)
		.ok_or(PacketError::MissingHeader(CLIENT_VERSION_HN.to_owned()))?;

	if client_version != env!("CARGO_PKG_VERSION") {
		Err(PacketError::InvalidVersion(
			client_version
				.to_str()
				.unwrap_or("Unknown version")
				.to_owned(),
		))
	} else {
		Ok(())
	}
}

async fn packet_validation_middleware(req: Request<Body>, next: Next) -> Response<Body> {
	match validate_packet(req.headers()) {
		Ok(_) => next.run(req).await,
		Err(err) => match err {
			PacketError::MissingHeader(header_name) => (
				StatusCode::BAD_REQUEST,
				axum::Json(serde_json::json!({
					"message": "Header missing from packet.",
					"header_name": header_name.as_str()
				})),
			)
				.into_response(),
			PacketError::InvalidVersion(_) => (
				StatusCode::UPGRADE_REQUIRED,
				axum::Json(serde_json::json!({
					"message": "Invalid client version.",
					"required_version": env!("CARGO_PKG_VERSION")
				})),
			)
				.into_response(),
		},
	}
}

#[derive(Debug)]
#[allow(unused)]
enum EasyManagerError {
	Socket(io::Error),
	Database(sea_orm::DbErr),
	Http(io::Error),
}

#[tracing::instrument]
#[tokio::main]
async fn main() -> Result<(), EasyManagerError> {
	let _logging_guard = init_logging();

	// Connect to database and migrate
	let db_connection: DatabaseConnection =
		sea_orm::Database::connect(std::env::var("DATABASE_URL").expect("DATABASE_URL is not set"))
			.await
			.map_err(|err| {
				tracing::error!(%err, "Failed to connect to database");
				EasyManagerError::Database(err)
			})?;

	tracing::debug!("Connected to database");

	migration::Migrator::up(&db_connection, None)
		.await
		.map_err(|err| {
			tracing::error!(%err, "Failed to make migration on database");
			EasyManagerError::Database(err)
		})?;

	tracing::debug!("Migration complete");

	// Create default admin user
	create_user_unchecked(
		&db_connection,
		CreateUserRequest {
			email: "admin@admin.com".to_owned(),
			username: "admin".to_owned(),
			password: "admin".to_owned(),
			permission_level: PermissionLevel::Admin,
		},
	)
	.await;

	// Define HTTP handlers
	let auth_router: Router = Router::new()
		.route("/model", method_routing::post(endpoints::create_item_model))
		.route("/user", method_routing::post(endpoints::create_user))
		.layer(middleware::from_fn_with_state(
			db_connection.clone(),
			auth_middleware,
		))
		.with_state(db_connection.clone());

	let unauth_router: Router = Router::new()
		.route("/ping", method_routing::get(endpoints::ping))
		.route(
			"/user/{user_id}",
			method_routing::get(endpoints::get_public_user_info),
		)
		.route(
			"/logged_in_user",
			method_routing::get(endpoints::get_logged_in_user),
		)
		.route("/users", method_routing::get(endpoints::get_users))
		.route("/items", method_routing::get(endpoints::get_items))
		.route("/login", method_routing::post(endpoints::login))
		.route(
			"/login_using_perm_token",
			method_routing::post(endpoints::login_using_permanent_token),
		)
		.route("/logout", method_routing::post(endpoints::logout))
		.route(
			"/obtain_permanent_token",
			method_routing::post(endpoints::obtain_permanent_token),
		)
		.with_state(db_connection.clone());

	let main_router = auth_router
		.merge(unauth_router)
		.layer(middleware::from_fn(packet_validation_middleware))
		.fallback(async |uri: Uri| -> (StatusCode, ResponsePacket<()>) {
			(
				StatusCode::NOT_FOUND,
				Packet::Error(ErrorPacket {
					message: format!("Route not found for {uri}"),
				})
				.into(),
			)
		});

	// Open port 3000
	let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
		.await
		.map_err(|err| {
			tracing::error!(%err, "Failed to start TCP listener");
			EasyManagerError::Socket(err)
		})?;

	tracing::info!("Listening on 3000");

	// Put HTTP router on 3000
	axum::serve(listener, main_router).await.map_err(|err| {
		tracing::error!(%err);
		EasyManagerError::Http(err)
	})
}
