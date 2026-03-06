use crate::entity::prelude::{Session, User};
use crate::entity::{session, user};
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{Error, SaltString};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use easy_manager_core::AccountIdentifier;
use easy_manager_core::packets::post::{CreateUserRequest, LoginRequest};
use easy_manager_core::packets::{ErrorPacket, Packet};
use migration::{Expr, Value};
use once_cell::sync::Lazy;
use regex::Regex;
use sea_orm::ColumnTrait;
use sea_orm::QueryFilter;
use sea_orm::{EntityTrait, NotSet, Set};
use uuid::Uuid;

static EMAIL_REGEX: Lazy<Regex> = Lazy::new(|| {
	Regex::new(r#"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}"#)
		.expect("Invalid email regex pattern")
});

static DUMMY_PASSWORD_HASH: Lazy<String> = Lazy::new(|| {
	let salt: SaltString = SaltString::generate(&mut OsRng);
	Argon2::default()
		.hash_password("dummy".as_bytes(), &salt)
		.expect("Failed to generate a dummy password hash")
		.to_string()
});

#[tracing::instrument]
pub async fn create_user(
	state: State<sea_orm::DatabaseConnection>,
	Json(create_user_request): Json<CreateUserRequest>,
) -> (StatusCode, Packet<()>) {
	// todo add proper validation for this and other functions

	if !EMAIL_REGEX.is_match(create_user_request.email.as_str()) {
		return (
			StatusCode::BAD_REQUEST,
			Packet::Error(ErrorPacket {
				message: "Invalid email".to_owned(),
			}),
		);
	}

	let password_salt: SaltString = SaltString::generate(&mut OsRng);
	let password_hash: String = match Argon2::default()
		.hash_password(create_user_request.password.as_bytes(), &password_salt)
	{
		Ok(hash) => hash.to_string(),
		Err(err) => {
			tracing::error!(%err, "Failed to hash password");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				Packet::Error(ErrorPacket {
					message: "Something went wrong".to_owned(),
				}),
			);
		}
	};

	match User::insert(user::ActiveModel {
		user_id: Set(Uuid::new_v4()),
		email: Set(create_user_request.email),
		username: Set(create_user_request.username.clone()),
		password: Set(password_hash),
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

	(StatusCode::CREATED, Packet::Ok(()))
}

#[tracing::instrument]
pub async fn login(
	state: State<sea_orm::DatabaseConnection>,
	cookie_jar: CookieJar,
	Json(login_request): Json<LoginRequest>,
) -> (StatusCode, CookieJar, Packet<()>) {
	let mut user_found = true;

	let user: user::Model = match User::find()
		.filter(match login_request.account_identifier {
			AccountIdentifier::Email { email } => user::Column::Email.eq(email),
			AccountIdentifier::Username { username } => user::Column::Username.eq(username),
		})
		.one(&state.0)
		.await
	{
		// a redundant password check should be made to prevent timing attacks
		Ok(user) => user.unwrap_or_else(|| {
			user_found = false;
			user::Model {
				user_id: Uuid::new_v4(),
				email: "email@email.com".to_owned(),
				username: "lmao".to_owned(),
				password: DUMMY_PASSWORD_HASH.clone(),
				created_at: Default::default(),
			}
		}),
		Err(err) => {
			tracing::error!(%err, "Failed to fetch user record from database");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				cookie_jar,
				Packet::Error(ErrorPacket {
					message: "Something went wrong".to_owned(),
				}),
			);
		}
	};

	let password_hash = match PasswordHash::new(user.password.as_str()) {
		Ok(password_hash) => password_hash,
		Err(err) => {
			tracing::error!(%err, "Failed to parse password into hash");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				cookie_jar,
				Packet::Error(ErrorPacket {
					message: "Something went wrong".to_owned(),
				}),
			);
		}
	};
	if let Err(err) =
		Argon2::default().verify_password(login_request.password.as_bytes(), &password_hash)
	{
		return match err {
			Error::Password => (
				StatusCode::UNAUTHORIZED,
				cookie_jar,
				Packet::Error(ErrorPacket {
					message: "Invalid username or password".to_owned(),
				}),
			),
			_ => {
				tracing::error!(%err, "Failed to verify password");
				(
					StatusCode::INTERNAL_SERVER_ERROR,
					cookie_jar,
					Packet::Error(ErrorPacket {
						message: "Something went wrong".to_owned(),
					}),
				)
			}
		};
	}

	// Case for if the user isn't found but the password they entered was the same as the dummy
	if !user_found {
		return (
			StatusCode::UNAUTHORIZED,
			cookie_jar,
			Packet::Error(ErrorPacket {
				message: "Invalid username or password".to_owned(),
			}),
		);
	}

	// User is now authenticated

	if let Err(err) = Session::update_many()
		.col_expr(
			session::Column::Expired,
			Expr::value(Value::Bool(Some(true))),
		)
		.filter(session::Column::UserId.eq(user.user_id))
		.exec(&state.0)
		.await
	{
		tracing::error!(%err, "Failed to update old sessions");
		return (
			StatusCode::INTERNAL_SERVER_ERROR,
			cookie_jar,
			Packet::Error(ErrorPacket {
				message: "Something went wrong".to_owned(),
			}),
		);
	}

	let insert = match Session::insert(session::ActiveModel {
		session_id: Set(Uuid::new_v4()),
		user_id: Set(user.user_id),
		created_at: NotSet,
		expired: NotSet,
	})
	.exec(&state.0)
	.await
	{
		Ok(insert) => insert,
		Err(err) => {
			tracing::error!(%err, "Failed to insert new session into database");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				cookie_jar,
				Packet::Error(ErrorPacket {
					message: "Something went wrong".to_owned(),
				}),
			);
		}
	};

	let cookie_jar = cookie_jar.add(
		Cookie::build(("session", insert.last_insert_id.to_string()))
			.path("/")
			.http_only(true)
			.secure(std::env::var("IS_DEV").map(|_| false).unwrap_or(true))
			.same_site(SameSite::Lax),
	);

	(StatusCode::CREATED, cookie_jar, Packet::Ok(()))
}

#[tracing::instrument]
pub async fn ping() -> StatusCode {
	StatusCode::OK
}

#[tracing::instrument]
pub async fn test() -> StatusCode {
	StatusCode::OK
}
