use axum::response::IntoResponse;
use easy_manager_core::packets::PacketError;
use migration::MigratorTrait;
use std::io;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

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

async fn get_device_info() -> (
	axum::http::StatusCode,
	axum::Json<easy_manager_core::packets::get::DeviceInfo>,
) {
	(
		axum::http::StatusCode::OK,
		axum::Json(easy_manager_core::packets::get::DeviceInfo {}),
	)
}

async fn ping() -> axum::http::StatusCode {
	axum::http::StatusCode::OK
}

async fn packet_validation_middleware(
	req: axum::http::Request<axum::body::Body>,
	next: axum::middleware::Next,
) -> axum::http::Response<axum::body::Body> {
	if req.uri() == "/ping" {
		return next.run(req).await
	}
	match easy_manager_core::packets::validate_packet(req.headers()) {
		Ok(_) => next.run(req).await,
		Err(err) => match err {
			PacketError::MissingHeaderError(header_name) => (
				axum::http::StatusCode::BAD_REQUEST,
				axum::Json(serde_json::json!({
					"message": "Header missing from packet.",
					"header_name": header_name.as_str()
				})),
			)
				.into_response(),
			PacketError::InvalidVersion(_) => (
				axum::http::StatusCode::UPGRADE_REQUIRED,
				axum::Json(serde_json::json!({
					"message": "Invalid client version.",
					"required_version": env!("CARGO_PKG_VERSION")
				})),
			)
				.into_response(),
		},
	}
}

async fn generate_session(user_id: u64) -> u64 {
	todo!()
}

#[derive(Debug)]
enum EasyManagerError {
	SocketError(io::Error),
	DatabaseError(sea_orm::DbErr),
	HTTPError(io::Error),
}

#[tracing::instrument]
#[tokio::main]
async fn main() -> Result<(), EasyManagerError> {
	let _logging_guard = init_logging();

	let router: axum::Router = axum::Router::new()
		.route("/", axum::routing::method_routing::get(get_device_info))
		.route("/ping", axum::routing::method_routing::get(ping))
		.layer(axum::middleware::from_fn(packet_validation_middleware));

	let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
		.await
		.map_err(|err| {
			tracing::error!(%err, "Failed to start TCP listener");
			EasyManagerError::SocketError(err)
		})?;

	let db_connection: sea_orm::DatabaseConnection =
		sea_orm::Database::connect(std::env::var("DATABASE_URL").expect("DATABASE_URL is not set"))
			.await
			.map_err(|err| {
				tracing::error!(%err, "Failed to connect to database");
				EasyManagerError::DatabaseError(err)
			})?;

	migration::Migrator::up(&db_connection, None)
		.await
		.map_err(|err| {
			tracing::error!(%err, "Failed to make migration on database");
			EasyManagerError::DatabaseError(err)
		})?;

	tracing::info!("Listening on 3000");

	axum::serve(listener, router).await.map_err(|err| {
		tracing::error!(%err);
		EasyManagerError::HTTPError(err)
	})
}
