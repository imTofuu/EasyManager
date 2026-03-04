use crate::entity;
use crate::entity::session;
use crate::entity::user::ActiveModel;
use axum::http::{HeaderMap, StatusCode};
use easy_manager_core::packets::post::{CreateUserRequest, CreateUserResponse, GenerateSessionResponse};
use easy_manager_core::packets::{ErrorPacket, Packet};
use migration::{Expr, Value};
use sea_orm::QueryFilter;
use sea_orm::{ColumnTrait, InsertResult};
use sea_orm::{EntityTrait, NotSet, Set};
use std::str::FromStr;

#[tracing::instrument]
pub async fn create_user(
	state: axum::extract::State<sea_orm::DatabaseConnection>,
	_headers: HeaderMap,
	axum::Json(create_user_request): axum::Json<CreateUserRequest>,
) -> (StatusCode, Packet<CreateUserResponse>) {
	let insert: InsertResult<ActiveModel> = match entity::prelude::User::insert(ActiveModel {
		user_id: Set(uuid::Uuid::new_v4()),
		username: Set(create_user_request.username),
		password_hash: Set(create_user_request.password_hash),
		created_at: Default::default(),
	})
	.exec(&state.0)
	.await
	{
		Ok(insert_result) => insert_result,
		Err(err) => {
			tracing::error!(%err, "Failed to insert into database");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				Packet::Error(ErrorPacket {
					message: "Failed to insert into database".to_owned(),
				}),
			);
		}
	};
	(
		StatusCode::CREATED,
		Packet::Ok(CreateUserResponse {
			user_id: insert.last_insert_id.to_string(),
		}),
	)
}

#[tracing::instrument]
pub async fn generate_session(
	state: axum::extract::State<sea_orm::DatabaseConnection>,
	headers: HeaderMap,
) -> (StatusCode, Packet<GenerateSessionResponse>) {
	// todo make this safe
	let user_id = uuid::Uuid::from_str(
		headers
			.get(axum::http::header::AUTHORIZATION)
			.unwrap()
			.to_str()
			.unwrap(),
	)
	.unwrap();
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

	let response = GenerateSessionResponse {
		session_id: insert.last_insert_id.to_string(),
	};

	(StatusCode::CREATED, Packet::Ok(response))
}

#[tracing::instrument]
pub async fn ping() -> StatusCode {
	StatusCode::OK
}

#[tracing::instrument]
pub async fn test() -> StatusCode {
	StatusCode::OK
}
