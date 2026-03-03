use std::str::FromStr;
use crate::entity;
use crate::entity::{session, user};
use easy_manager_core::packets::Packet;
use easy_manager_core::packets::post::{CreateUserRequest, CreateUserResponse, GenerateSession};
use migration::{Expr, Value};
use sea_orm::ColumnTrait;
use sea_orm::QueryFilter;
use sea_orm::{EntityTrait, NotSet, Set};

pub async fn create_user(
	state: axum::extract::State<sea_orm::DatabaseConnection>,
	_headers: axum::http::HeaderMap,
	axum::Json(create_user_request): axum::Json<CreateUserRequest>
) -> (axum::http::StatusCode, Packet<CreateUserResponse>) {
	// todo make this safe
	let insert = entity::prelude::User::insert(user::ActiveModel {
		user_id: Set(uuid::Uuid::new_v4()),
		username: Set(create_user_request.username),
		password_hash: Set(create_user_request.password_hash),
		created_at: Default::default(),
	}).exec(&state.0).await.unwrap();
	(axum::http::StatusCode::CREATED, Packet::Ok(CreateUserResponse {
		user_id: insert.last_insert_id.to_string()
	}))
}

pub async fn generate_session(
	state: axum::extract::State<sea_orm::DatabaseConnection>,
	req: axum::http::Request<axum::body::Body>,
) -> (axum::http::StatusCode, Packet<GenerateSession>) {
	// todo make this safe
	let user_id = uuid::Uuid::from_str(req
		.headers()
		.get(axum::http::header::AUTHORIZATION)
		.unwrap().to_str().unwrap()).unwrap();
	let _ = entity::prelude::Session::update_many()
		.col_expr(
			session::Column::Expired,
			Expr::value(Value::Bool(Some(true))),
		)
		.filter(session::Column::UserId.eq(user_id))
		.exec(&state.0)
		.await;
	let insert = entity::prelude::Session::insert(session::ActiveModel {
		session_id: Set(uuid::Uuid::new_v4()),
		user_id: Set(user_id),
		created_at: NotSet,
		expired: NotSet,
	})
	.exec(&state.0)
	.await
	.unwrap();

	let response = GenerateSession {
		session_id: insert.last_insert_id.to_string(),
	};

	(axum::http::StatusCode::CREATED, Packet::Ok(response))
}

pub async fn ping() -> axum::http::StatusCode {
	axum::http::StatusCode::OK
}
