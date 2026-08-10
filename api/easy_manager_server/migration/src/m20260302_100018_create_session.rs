use sea_orm_migration::prelude::*;

use crate::m20220101_000001_create_users::User;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(Iden)]
enum Session {
	Table,
	SessionId,
	UserId,
	CreatedAt,
	Expired,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_table(
				Table::create()
					.table(Session::Table)
					.if_not_exists()
					.col(
						ColumnDef::new(Session::SessionId)
							.uuid()
							.primary_key()
							.not_null(),
					)
					.col(ColumnDef::new(Session::UserId).uuid().not_null())
					.col(
						ColumnDef::new(Session::CreatedAt)
							.timestamp_with_time_zone()
							.default(Expr::current_timestamp())
							.not_null(),
					)
					.col(
						ColumnDef::new(Session::Expired)
							.boolean()
							.default(false)
							.not_null(),
					)
					.foreign_key(
						ForeignKey::create()
							.name("fk-session-userid")
							.from(Session::Table, Session::UserId)
							.to(User::Table, User::UserId),
					)
					.to_owned(),
			)
			.await
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.drop_table(Table::drop().table(Session::Table).to_owned())
			.await
	}
}
