use std::str::FromStr;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{Error, SaltString};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use easy_manager_core::packets::get::{
	GetItemInfoResponse, GetItemsResponse, GetUserInfoResponse, GetUsersResponse,
};
use easy_manager_core::packets::post::{
	BorrowRequest, CreateItemModelRequest, CreateUserRequest, LoginRequest, LoginResponse,
	LoginUsingPermanentTokenRequest, ObtainPermanentTokenRequest, ObtainPermanentTokenResponse,
	ReturnRequest,
};
use easy_manager_core::packets::{ErrorPacket, Packet};
use easy_manager_core::{AccountIdentifier, PermissionLevel};
use migration::{Expr, IntoCondition, JoinType};
use once_cell::sync::Lazy;
use regex::Regex;
use sea_orm::{
	ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, InsertResult,
	IntoActiveModel, NotSet, QueryFilter, QuerySelect, RelationTrait, Set,
};
use uuid::Uuid;

use crate::ResponsePacket;
use crate::entity::prelude::{Borrow, Item, ItemModel, PermanentToken, Session, User};
use crate::entity::{borrow, item, item_model, permanent_token, session, user};

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
pub async fn get_public_user_info(
	state: State<DatabaseConnection>,
	Path(user_id): Path<Uuid>,
) -> (StatusCode, ResponsePacket<GetUserInfoResponse>) {
	let user: user::Model = match User::find_by_id(user_id).one(&state.0).await {
		Ok(user) => match user {
			Some(user) => user,
			None => {
				tracing::error!("I dont event know what happened here");
				return (
					StatusCode::INTERNAL_SERVER_ERROR,
					Packet::Error(
						StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
						ErrorPacket {
							message: "Something went wrong".to_owned(),
						},
					)
					.into(),
				);
			}
		},
		Err(err) => {
			tracing::error!(%err, "Failed to get user from session model");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong".to_owned(),
					},
				)
				.into(),
			);
		}
	};

	let permission_level = match user.permission_level.try_into() {
		Ok(permission_level) => permission_level,
		Err(err) => {
			tracing::error!(%err, "Permission level of user is malformed");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong".to_owned(),
					},
				)
				.into(),
			);
		}
	};

	(
		StatusCode::OK,
		Packet::Ok(
			StatusCode::OK.as_u16(),
			GetUserInfoResponse {
				username: user.username,
				user_id: user.user_id.to_string(),
				permission_level,
			},
		)
		.into(),
	)
}

//todo make this not public
#[tracing::instrument]
pub async fn get_users(
	state: State<DatabaseConnection>,
) -> (StatusCode, ResponsePacket<GetUsersResponse>) {
	let users = match User::find().all(&state.0).await {
		Ok(ok) => ok,
		Err(err) => {
			tracing::error!(%err, "Failed to get all users from database");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong".to_owned(),
					},
				)
				.into(),
			);
		}
	};

	let users: Box<[GetUserInfoResponse]> = match users
		.into_iter()
		.map(|user| {
			Ok::<GetUserInfoResponse, String>(GetUserInfoResponse {
				username: user.username,
				user_id: user.user_id.to_string(),
				permission_level: user.permission_level.try_into()?,
			})
		})
		.collect()
	{
		Ok(slice) => slice,
		Err(err) => {
			tracing::error!(%err, "Failed to collect all users");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong".to_owned(),
					},
				)
				.into(),
			);
		}
	};

	(
		StatusCode::OK,
		Packet::Ok(StatusCode::OK.as_u16(), GetUsersResponse { users }).into(),
	)
}

#[tracing::instrument]
pub async fn get_logged_in_user(
	state: State<DatabaseConnection>,
	cookie_jar: CookieJar,
	Extension(session): Extension<session::Model>,
) -> (StatusCode, CookieJar, ResponsePacket<GetUserInfoResponse>) {
	let (code, response) = get_public_user_info(state, Path(session.user_id)).await;

	(code, cookie_jar, response)
}

#[tracing::instrument]
pub async fn get_item(
	state: State<DatabaseConnection>,
	Path(item_id): Path<Uuid>,
) -> (StatusCode, ResponsePacket<GetItemInfoResponse>) {
	let (item_record, borrow_record) = match Item::find_by_id(item_id)
		.join(
			JoinType::LeftJoin,
			item::Relation::Borrow.def().on_condition(|_left, right| {
				Expr::col((right, borrow::Column::Returned))
					.eq(false)
					.into_condition()
			}),
		)
		.select_also(Borrow)
		.one(&state.0)
		.await
	{
		Ok(Some(record)) => record,
		Ok(None) => {
			return (
				StatusCode::BAD_REQUEST,
				Packet::Error(
					StatusCode::BAD_REQUEST.as_u16(),
					ErrorPacket {
						message: "Invalid item".to_owned(),
					},
				)
				.into(),
			);
		}
		Err(err) => {
			tracing::error!(%err, "Failed to get item from database");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong".to_owned(),
					},
				)
				.into(),
			);
		}
	};

	let permission_level = match item_record.permission_level.map(String::try_into) {
		Some(Ok(level)) => level,
		Some(Err(_)) => {
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong".to_owned(),
					},
				)
				.into(),
			);
		}
		None => {
			match ItemModel::find_by_id(item_record.item_model_id)
				.one(&state.0)
				.await
			{
				Ok(Some(model)) => match model.permission_level.try_into() {
					Ok(uuid) => uuid,
					Err(err) => {
						tracing::error!(%err, "Malformed permission level stored in item model database");
						return (
							StatusCode::INTERNAL_SERVER_ERROR,
							Packet::Error(
								StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
								ErrorPacket {
									message: "Something went wrong".to_owned(),
								},
							)
							.into(),
						);
					}
				},
				Ok(None) => {
					tracing::error!("Invalid item model id stored in item record");
					return (
						StatusCode::INTERNAL_SERVER_ERROR,
						Packet::Error(
							StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
							ErrorPacket {
								message: "Something went wrong".to_owned(),
							},
						)
						.into(),
					);
				}
				Err(err) => {
					tracing::error!(%err, "Couldn't get item model from database");
					return (
						StatusCode::INTERNAL_SERVER_ERROR,
						Packet::Error(
							StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
							ErrorPacket {
								message: "Something went wrong".to_owned(),
							},
						)
						.into(),
					);
				}
			}
		}
	};

	(
		StatusCode::OK,
		Packet::Ok(
			StatusCode::OK.as_u16(),
			GetItemInfoResponse {
				name: item_record.name,
				item_id: item_record.item_id.to_string(),
				permission_level,
				item_model_id: item_record.item_model_id.to_string(),
				borrow_id: borrow_record.map(|record| record.borrow_id.to_string()),
			},
		)
		.into(),
	)
}

#[tracing::instrument]
pub async fn get_items(
	state: State<DatabaseConnection>,
) -> (StatusCode, ResponsePacket<GetItemsResponse>) {
	let items = match Item::find()
		.join(
			JoinType::LeftJoin,
			item::Relation::Borrow.def().on_condition(|_left, right| {
				Expr::col((right, borrow::Column::Returned))
					.eq(false)
					.into_condition()
			}),
		)
		.select_also(Borrow)
		.join(
			JoinType::LeftJoin,
			item::Relation::ItemModel
				.def()
				.on_condition(|left, _right| {
					Expr::col((left, item::Column::PermissionLevel))
						.is_null()
						.into_condition()
				}),
		)
		.select_also(ItemModel)
		.all(&state.0)
		.await
	{
		Ok(ok) => ok,
		Err(err) => {
			tracing::error!(%err, "Failed to get all items from database");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong".to_owned(),
					},
				)
				.into(),
			);
		}
	};

	let items: Box<[GetItemInfoResponse]> = match items
		.into_iter()
		.map(|(item_record, borrow_record, item_model_record)| {
			let permission_level = match item_record.permission_level.map(String::try_into) {
				Some(Ok(level)) => level,
				Some(Err(err)) => return Err(err),
				None => match item_model_record {
					Some(record) => record.permission_level.try_into()?,
					None => return Err("Invalid item model id in item".to_owned()),
				},
			};
			Ok(GetItemInfoResponse {
				name: item_record.name,
				item_id: item_record.item_id.to_string(),
				permission_level,
				item_model_id: item_record.item_model_id.to_string(),
				borrow_id: borrow_record.map(|record| record.borrow_id.to_string()),
			})
		})
		.collect()
	{
		Ok(items) => items,
		Err(err) => {
			tracing::error!(%err);
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong".to_owned(),
					},
				)
				.into(),
			);
		}
	};

	(
		StatusCode::OK,
		Packet::Ok(StatusCode::OK.as_u16(), GetItemsResponse { items }).into(),
	)
}

#[tracing::instrument]
pub async fn create_item_model(
	state: State<DatabaseConnection>,
	cookie_jar: CookieJar,
	Extension(session): Extension<session::Model>,
	Json(create_item_model_request): Json<CreateItemModelRequest>,
) -> (StatusCode, CookieJar, ResponsePacket<()>) {
	let (get_user_info_status_code, user_info) =
		get_public_user_info(state.clone(), Path(session.user_id)).await;

	match user_info.0 {
		Packet::Ok(_, get_user_info_response) => {
			if get_user_info_response.permission_level < PermissionLevel::Admin {
				return (
					StatusCode::FORBIDDEN,
					cookie_jar,
					Packet::Error(
						StatusCode::FORBIDDEN.as_u16(),
						ErrorPacket {
							message: "Insufficient permissions".to_owned(),
						},
					)
					.into(),
				);
			}
		}
		Packet::Error(code, err) => {
			return (
				get_user_info_status_code,
				cookie_jar,
				Packet::Error(code, err).into(),
			);
		}
	}

	if let Err(err) = ItemModel::insert(item_model::ActiveModel {
		item_model_id: Set(Uuid::new_v4()),
		name: Set(create_item_model_request.name),
		description: Set(create_item_model_request.description),
		permission_level: Set(create_item_model_request.permission_level.into()),
	})
	.exec(&state.0)
	.await
	{
		tracing::error!(%err, "Failed to insert item model into database");
		return (
			StatusCode::INTERNAL_SERVER_ERROR,
			cookie_jar,
			Packet::Error(
				StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
				ErrorPacket {
					message: "Something went wrong".to_owned(),
				},
			)
			.into(),
		);
	}

	(
		StatusCode::CREATED,
		cookie_jar,
		Packet::Ok(StatusCode::CREATED.as_u16(), ()).into(),
	)
}

#[tracing::instrument]
pub async fn create_user_unchecked(
	db: &DatabaseConnection,
	create_user_request: CreateUserRequest,
) -> (StatusCode, ResponsePacket<()>) {
	if !EMAIL_REGEX.is_match(create_user_request.email.as_str()) {
		return (
			StatusCode::BAD_REQUEST,
			Packet::Error(
				StatusCode::BAD_REQUEST.as_u16(),
				ErrorPacket {
					message: "Invalid email".to_owned(),
				},
			)
			.into(),
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
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong".to_owned(),
					},
				)
				.into(),
			);
		}
	};

	match User::insert(user::ActiveModel {
		user_id: Set(Uuid::new_v4()),
		email: Set(create_user_request.email),
		username: Set(create_user_request.username.clone()),
		password: Set(password_hash),
		permission_level: Set(create_user_request.permission_level.into()),
		created_at: Default::default(),
	})
	.exec(db)
	.await
	{
		Ok(insert_result) => insert_result,
		Err(err) => {
			tracing::error!(%err, "Failed to insert into database");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Failed to insert into database".to_owned(),
					},
				)
				.into(),
			);
		}
	};

	(
		StatusCode::CREATED,
		Packet::Ok(StatusCode::CREATED.as_u16(), ()).into(),
	)
}

#[tracing::instrument]
pub async fn create_user(
	state: State<DatabaseConnection>,
	Extension(session): Extension<session::Model>,
	Json(create_user_request): Json<CreateUserRequest>,
) -> (StatusCode, ResponsePacket<()>) {
	// todo add proper validation for this and other functions (db insertions)

	let (status_code, user_info) = get_public_user_info(state.clone(), Path(session.user_id)).await;

	match user_info.0 {
		Packet::Ok(_, get_user_info_response) => {
			if get_user_info_response.permission_level < PermissionLevel::Admin {
				return (
					StatusCode::FORBIDDEN,
					Packet::Error(
						StatusCode::FORBIDDEN.as_u16(),
						ErrorPacket {
							message: "Insufficient permissions".to_owned(),
						},
					)
					.into(),
				);
			}
		}
		Packet::Error(code, err) => return (status_code, Packet::Error(code, err).into()),
	}

	create_user_unchecked(&state.0, create_user_request).await
}

#[tracing::instrument]
pub async fn logout(
	state: State<DatabaseConnection>,
	cookie_jar: CookieJar,
) -> (StatusCode, CookieJar) {
	let session_id: Option<Uuid> = match cookie_jar.get("session") {
		Some(session_id) => Uuid::from_str(session_id.value()).ok(),
		None => return (StatusCode::NO_CONTENT, cookie_jar),
	};

	if let Some(session_id) = session_id
		&& let Err(err) = Session::update_many()
			.col_expr(session::Column::Expired, Expr::value(true))
			.filter(session::Column::SessionId.eq(session_id))
			.exec(&state.0)
			.await
	{
		tracing::error!(%err, "Failed to mark old session as expired");
	}

	let cookie_jar = cookie_jar.remove(Cookie::build("session").path("/"));
	(StatusCode::NO_CONTENT, cookie_jar)
}

#[tracing::instrument]
pub async fn push_session(
	state: State<DatabaseConnection>,
	cookie_jar: CookieJar,
	user_id: Uuid,
) -> Result<(CookieJar, InsertResult<session::ActiveModel>), DbErr> {
	let (_, cookie_jar) = logout(state.clone(), cookie_jar).await;

	let insert = Session::insert(session::ActiveModel {
		session_id: Set(Uuid::new_v4()),
		user_id: Set(user_id),
		created_at: NotSet,
		expired: NotSet,
	})
	.exec(&state.0)
	.await?;

	let session_id = insert.last_insert_id.clone();

	let cookie_jar = cookie_jar.add(
		Cookie::build(("session", session_id.to_string()))
			.path("/")
			.http_only(true)
			.secure(std::env::var("IS_DEV").map(|_| false).unwrap_or(true))
			.same_site(SameSite::Lax),
	);

	Ok((cookie_jar, insert))
}

#[tracing::instrument]
pub async fn login(
	state: State<DatabaseConnection>,
	cookie_jar: CookieJar,
	Json(login_request): Json<LoginRequest>,
) -> (StatusCode, CookieJar, ResponsePacket<LoginResponse>) {
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
				permission_level: PermissionLevel::Default.into(),
				created_at: Default::default(),
			}
		}),
		Err(err) => {
			tracing::error!(%err, "Failed to fetch user record from database");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				cookie_jar,
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong".to_owned(),
					},
				)
				.into(),
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
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong".to_owned(),
					},
				)
				.into(),
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
				Packet::Error(
					StatusCode::UNAUTHORIZED.as_u16(),
					ErrorPacket {
						message: "Invalid username or password".to_owned(),
					},
				)
				.into(),
			),
			_ => {
				tracing::error!(%err, "Failed to verify password");
				(
					StatusCode::INTERNAL_SERVER_ERROR,
					cookie_jar,
					Packet::Error(
						StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
						ErrorPacket {
							message: "Something went wrong".to_owned(),
						},
					)
					.into(),
				)
			}
		};
	}

	// Case for if the user isn't found but the password they entered was the same
	// as the dummy
	if !user_found {
		return (
			StatusCode::UNAUTHORIZED,
			cookie_jar,
			Packet::Error(
				StatusCode::UNAUTHORIZED.as_u16(),
				ErrorPacket {
					message: "Invalid username or password".to_owned(),
				},
			)
			.into(),
		);
	}

	// User is now authenticated

	let (cookie_jar, session) = match push_session(state, cookie_jar.clone(), user.user_id).await {
		Ok(result) => result,
		Err(err) => {
			tracing::error!(%err, "Failed to push session");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				cookie_jar,
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong".to_owned(),
					},
				)
				.into(),
			);
		}
	};

	(
		StatusCode::CREATED,
		cookie_jar,
		Packet::Ok(
			StatusCode::CREATED.as_u16(),
			LoginResponse {
				session_id: session.last_insert_id.to_string(),
			},
		)
		.into(),
	)
}

#[tracing::instrument]
pub async fn login_using_permanent_token(
	state: State<DatabaseConnection>,
	cookie_jar: CookieJar,
	Json(login_request): Json<LoginUsingPermanentTokenRequest>,
) -> (StatusCode, CookieJar, ResponsePacket<LoginResponse>) {
	let token_model = match PermanentToken::find_by_id(Uuid::from_u128(login_request.token))
		.one(&state.0)
		.await
	{
		Ok(Some(user_id)) => user_id,
		Ok(None) => {
			return (
				StatusCode::BAD_REQUEST,
				cookie_jar,
				Packet::Error(
					StatusCode::BAD_REQUEST.as_u16(),
					ErrorPacket {
						message: "Invalid token".to_owned(),
					},
				)
				.into(),
			);
		}
		Err(err) => {
			tracing::error!(%err, "Failed to get user id with permanent token");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				cookie_jar,
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong".to_owned(),
					},
				)
				.into(),
			);
		}
	};

	let (cookie_jar, session) =
		match push_session(state, cookie_jar.clone(), token_model.user_id).await {
			Ok(result) => result,
			Err(err) => {
				tracing::error!(%err, "Failed to push session");
				return (
					StatusCode::INTERNAL_SERVER_ERROR,
					cookie_jar,
					Packet::Error(
						StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
						ErrorPacket {
							message: "Something went wrong".to_owned(),
						},
					)
					.into(),
				);
			}
		};

	(
		StatusCode::CREATED,
		cookie_jar,
		Packet::Ok(
			StatusCode::CREATED.as_u16(),
			LoginResponse {
				session_id: session.last_insert_id.to_string(),
			},
		)
		.into(),
	)
}

//todo add auth
#[tracing::instrument]
pub async fn obtain_permanent_token(
	state: State<DatabaseConnection>,
	Extension(session): Extension<session::Model>,
	Json(request): Json<ObtainPermanentTokenRequest>,
) -> (StatusCode, ResponsePacket<ObtainPermanentTokenResponse>) {
	let user_id = match request.user_id {
		Some(user_id) => {
			let session_user =
				match get_public_user_info(state.clone(), Path(session.user_id)).await {
					(_, ResponsePacket(Packet::Ok(_, session_user))) => session_user,
					(code, ResponsePacket(Packet::Error(_, error))) => {
						return (code, Packet::Error(code.as_u16(), error).into());
					}
				};
			if session_user.permission_level >= PermissionLevel::Admin {
				match Uuid::try_parse(user_id.as_str()) {
					Ok(uuid) => uuid,
					Err(err) => {
						tracing::error!("Passed auth middleware but invalid user id");
						return (
							StatusCode::INTERNAL_SERVER_ERROR,
							Packet::Error(
								StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
								ErrorPacket {
									message: "Something went wrong".to_owned(),
								},
							)
							.into(),
						);
					}
				}
			} else {
				return (
					StatusCode::FORBIDDEN,
					Packet::Error(
						StatusCode::FORBIDDEN.as_u16(),
						ErrorPacket {
							message: "Insufficient permissions to make token for another user"
								.to_owned(),
						},
					)
					.into(),
				);
			}
		}
		None => session.user_id,
	};

	let uuid = Uuid::new_v4();
	match PermanentToken::insert(permanent_token::ActiveModel {
		token: Set(uuid.clone()),
		user_id: Set(user_id),
		enabled: Set(true),
	})
	.exec(&state.0)
	.await
	{
		Ok(model) => model,
		Err(DbErr::Exec(err)) => {
			tracing::error!(%err, "Failed to insert permanent token into database");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong. Probably an invalid user id".to_owned(),
					},
				)
				.into(),
			);
		}
		Err(err) => {
			tracing::error!(%err, "Failed to insert permanent token into database");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong.".to_owned(),
					},
				)
				.into(),
			);
		}
	};

	(
		StatusCode::CREATED,
		Packet::Ok(
			StatusCode::CREATED.as_u16(),
			ObtainPermanentTokenResponse {
				token: uuid.as_u128(),
			},
		)
		.into(),
	)
}

#[tracing::instrument]
pub async fn borrow(
	state: State<DatabaseConnection>,
	Extension(session): Extension<session::Model>,
	Json(borrow_request): Json<BorrowRequest>,
) -> (StatusCode, ResponsePacket<()>) {
	let user = match get_public_user_info(state.clone(), Path(session.user_id.clone())).await {
		(_, ResponsePacket(Packet::Ok(_, user))) => user,
		(StatusCode::INTERNAL_SERVER_ERROR, ResponsePacket(Packet::Error(..))) => {
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong".to_owned(),
					},
				)
				.into(),
			);
		}
		(_, ResponsePacket(Packet::Error(..))) => {
			return (
				StatusCode::BAD_REQUEST,
				Packet::Error(
					StatusCode::BAD_REQUEST.as_u16(),
					ErrorPacket {
						message: "Invalid user id".to_owned(),
					},
				)
				.into(),
			);
		}
	};

	let item_id = match Uuid::try_parse(borrow_request.item_id.as_str()) {
		Ok(item_id) => item_id,
		Err(_) => {
			return (
				StatusCode::BAD_REQUEST,
				Packet::Error(
					StatusCode::BAD_REQUEST.as_u16(),
					ErrorPacket {
						message: "Malformed item id".to_owned(),
					},
				)
				.into(),
			);
		}
	};

	match get_item(state.clone(), Path(item_id.clone())).await.1.0 {
		Packet::Ok(_, packet) => {
			if packet.permission_level > user.permission_level {
				return (
					StatusCode::FORBIDDEN,
					Packet::Error(
						StatusCode::FORBIDDEN.as_u16(),
						ErrorPacket {
							message: "Insufficient permissions".to_owned(),
						},
					)
					.into(),
				);
			}
		}
		Packet::Error(..) => {
			return (
				StatusCode::BAD_REQUEST,
				Packet::Error(
					StatusCode::BAD_REQUEST.as_u16(),
					ErrorPacket {
						message: "Invalid item id".to_owned(),
					},
				)
				.into(),
			);
		}
	}

	let active_model = borrow::ActiveModel {
		borrow_id: Set(Uuid::new_v4()),
		user_id: Set(session.user_id),
		item_id: Set(item_id),
		created_at: NotSet,
		returned: NotSet,
	};

	match Borrow::find()
		.filter(borrow::Column::ItemId.eq(item_id))
		.filter(borrow::Column::Returned.eq(false))
		.one(&state.0)
		.await
	{
		Ok(Some(_)) => {
			return (
				StatusCode::BAD_REQUEST,
				Packet::Error(
					StatusCode::BAD_REQUEST.as_u16(),
					ErrorPacket {
						message: "Item is already borrowed right now".to_owned(),
					},
				)
				.into(),
			);
		}
		Err(err) => {
			tracing::error!(%err, "Failed to check if item is already borrowed");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong".to_owned(),
					},
				)
				.into(),
			);
		}
		_ => {}
	}

	if let Err(err) = Borrow::insert(active_model).exec(&state.0).await {
		tracing::error!(%err, "Failed to insert borrow record");
		return (
			StatusCode::INTERNAL_SERVER_ERROR,
			Packet::Error(
				StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
				ErrorPacket {
					message: "Something went wrong".to_owned(),
				},
			)
			.into(),
		);
	}

	(
		StatusCode::CREATED,
		Packet::Ok(StatusCode::CREATED.as_u16(), ()).into(),
	)
}

#[tracing::instrument]
pub async fn return_item(
	state: State<DatabaseConnection>,
	Json(return_request): Json<ReturnRequest>,
) -> (StatusCode, ResponsePacket<()>) {
	let item_id = match Uuid::try_parse(return_request.item_id.as_str()) {
		Ok(item_id) => item_id,
		Err(_) => {
			return (
				StatusCode::BAD_REQUEST,
				Packet::Error(
					StatusCode::BAD_REQUEST.as_u16(),
					ErrorPacket {
						message: "Malformed item id".to_owned(),
					},
				)
				.into(),
			);
		}
	};

	match Borrow::find()
		.filter(borrow::Column::ItemId.eq(item_id))
		.filter(borrow::Column::Returned.eq(false))
		.one(&state.0)
		.await
	{
		Ok(Some(record)) => {
			let mut active_model = record.into_active_model();
			active_model.returned = Set(true);

			if let Err(err) = active_model.update(&state.0).await {
				tracing::error!(%err, "Failed to update borrow record");
				return (
					StatusCode::INTERNAL_SERVER_ERROR,
					Packet::Error(
						StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
						ErrorPacket {
							message: "Something went wrong".to_owned(),
						},
					)
					.into(),
				);
			}
		}
		Ok(None) => {
			return (
				StatusCode::NOT_MODIFIED,
				Packet::Error(
					StatusCode::NOT_MODIFIED.as_u16(),
					ErrorPacket {
						message: "Item does not need to be returned".to_owned(),
					},
				)
				.into(),
			);
		}
		Err(err) => {
			tracing::error!(%err, "Failed to get borrow from database");
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				Packet::Error(
					StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
					ErrorPacket {
						message: "Something went wrong".to_owned(),
					},
				)
				.into(),
			);
		}
	}

	(
		StatusCode::OK,
		Packet::Ok(StatusCode::OK.as_u16(), ()).into(),
	)
}

#[tracing::instrument]
pub async fn ping() -> StatusCode {
	StatusCode::OK
}
