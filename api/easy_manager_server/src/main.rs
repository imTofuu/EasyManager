use axum::response::IntoResponse;
use easy_manager_core::packets::PacketError;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tracing::instrument]
fn init_logging() -> tracing_appender::non_blocking::WorkerGuard {
	let mut exe_path: std::path::PathBuf =
		std::env::current_exe().expect("Could not find executable location");
	exe_path.pop();

	let (file_writer, file_guard) = tracing_appender::non_blocking(
		tracing_appender::rolling::daily(exe_path.join("logs"), "easy_manager_server")
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
			}
		));

	let cout_layer = tracing_subscriber::fmt::layer()
		.pretty()
		.with_ansi(true)
		.with_filter(tracing_subscriber::filter::Targets::new().with_default(
			tracing_subscriber::filter::LevelFilter::from_level(if cfg!(debug_assertions) {
				tracing::Level::DEBUG
			} else {
				tracing::Level::INFO
			})
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
	axum::Json<easy_manager_core::packets::get::DeviceInfo>
) {
	(
		axum::http::StatusCode::OK,
		axum::Json(easy_manager_core::packets::get::DeviceInfo {})
	)
}

async fn packet_validation_middleware(
	req: axum::http::Request<axum::body::Body>,
	next: axum::middleware::Next
) -> axum::http::Response<axum::body::Body> {
	match easy_manager_core::packets::validate_packet(req.headers()) {
		Ok(_) => next.run(req).await,
		Err(err) => {
			match err {
				PacketError::MissingHeaderError(header_name) => {
					(
						axum::http::StatusCode::BAD_REQUEST,
						axum::Json(serde_json::json!({
							"message": "Header missing from packet.",
							"header_name": header_name.as_str()
						}))
					)
						.into_response()
				}
				PacketError::InvalidVersion(_) => {
					(
						axum::http::StatusCode::UPGRADE_REQUIRED,
						axum::Json(serde_json::json!({
							"message": "Invalid client version.",
							"required_version": env!("CARGO_PKG_VERSION")
						}))
					)
						.into_response()
				}
			}
		}
	}
}

#[tracing::instrument]
#[tokio::main]
async fn main() {
	let _logging_guard = init_logging();

	let router: axum::Router = axum::Router::new()
		.route("/", axum::routing::method_routing::get(get_device_info))
		.layer(axum::middleware::from_fn(packet_validation_middleware));

	let listener = match tokio::net::TcpListener::bind("0.0.0.0:3000").await {
		Ok(listener) => listener,
		Err(err) => {
			tracing::error!(%err, "Failed to start TCP listener");
			return;
		}
	};

	tracing::info!("Listening on 3000");

	let _ = axum::serve(listener, router)
		.await
		.inspect_err(|err| tracing::error!(%err));
}
