#![warn(clippy::unwrap_used)]

mod endpoints;
mod entity;

use crate::entity::prelude::Session;
use axum::body::Body;
use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{Request, Response, StatusCode};
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::routing::method_routing;
use axum::{Router, middleware};
use easy_manager_core::packets::{ErrorPacket, Packet, PacketError, validate_packet};
use migration::MigratorTrait;
use sea_orm::{DatabaseConnection, EntityTrait};
use std::io;
use std::str::FromStr;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use uuid::Uuid;

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

#[tracing::instrument]
async fn auth_middleware(
	State(state): State<DatabaseConnection>,
	mut req: Request<Body>,
	next: Next,
) -> Response<Body> {
	let session_id: Uuid =
		match req.headers().get(AUTHORIZATION) {
			Some(session_id) => Uuid::from_str(session_id.to_str().unwrap()).unwrap(),
			None => return (
				StatusCode::UNAUTHORIZED,
				Packet::<()>::Error(ErrorPacket {
					message:
						"This endpoint requires a valid session token in the Authorization header"
							.to_owned(),
				}),
			)
				.into_response(),
		};
	match Session::find_by_id(session_id).one(&state).await {
		Ok(session) => match session {
			Some(session) => {
				if session.expired {
					return (
						StatusCode::FORBIDDEN,
						Packet::<()>::Error(ErrorPacket {
							message: "Session is expired".to_owned(),
						}),
					)
						.into_response();
				}
			}
			None => {
				return (
					StatusCode::FORBIDDEN,
					Packet::<()>::Error(ErrorPacket {
						message: "Session is not found".to_owned(),
					}),
				)
					.into_response();
			}
		},
		Err(err) => {
			tracing::error!(%err, "An error occurred validating a session");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				Packet::<()>::Error(ErrorPacket {
					message: "Something went wrong validating the session".to_owned(),
				}),
			)
				.into_response();
		}
	}
	req.extensions_mut().insert(session_id);
	next.run(req).await
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
	SocketError(io::Error),
	DatabaseError(sea_orm::DbErr),
	HTTPError(io::Error),
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
				EasyManagerError::DatabaseError(err)
			})?;

	tracing::debug!("Connected to database");

	migration::Migrator::up(&db_connection, None)
		.await
		.map_err(|err| {
			tracing::error!(%err, "Failed to make migration on database");
			EasyManagerError::DatabaseError(err)
		})?;

	tracing::debug!("Migration complete");

	// Define HTTP handlers
	let auth_router: Router = Router::new()
		.route("/test", method_routing::get(endpoints::test))
		.layer(middleware::from_fn_with_state(
			db_connection.clone(),
			auth_middleware,
		))
		.with_state(db_connection.clone());

	let unauth_router: Router = Router::new()
		.route("/ping", method_routing::get(endpoints::ping))
		.route("/user", method_routing::post(endpoints::create_user))
		.route(
			"/generate_session",
			method_routing::post(endpoints::generate_session),
		)
		.with_state(db_connection.clone());

	let main_router = auth_router
		.merge(unauth_router)
		.layer(middleware::from_fn(packet_validation_middleware));

	// Open port 3000
	let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
		.await
		.map_err(|err| {
			tracing::error!(%err, "Failed to start TCP listener");
			EasyManagerError::SocketError(err)
		})?;

	tracing::info!("Listening on 3000");

	// Put HTTP router on 3000
	axum::serve(listener, main_router).await.map_err(|err| {
		tracing::error!(%err);
		EasyManagerError::HTTPError(err)
	})
}
