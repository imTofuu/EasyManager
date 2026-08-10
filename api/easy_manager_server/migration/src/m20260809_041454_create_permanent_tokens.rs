use sea_orm_migration::prelude::*;

use crate::m20220101_000001_create_users::User;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(Iden)]
enum PermanentToken {
	Table,
	Token,
	UserId,
	Enabled,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_table(
				Table::create()
					.table(PermanentToken::Table)
					.if_not_exists()
					.col(
						ColumnDef::new(PermanentToken::Token)
							.uuid()
							.primary_key()
							.not_null(),
					)
					.col(ColumnDef::new(PermanentToken::UserId).uuid().not_null())
					.col(ColumnDef::new(PermanentToken::Enabled).boolean().not_null())
					.foreign_key(
						ForeignKey::create()
							.name("fk-permanenttoken-userid")
							.from(PermanentToken::Table, PermanentToken::UserId)
							.to(User::Table, User::UserId),
					)
					.to_owned(),
			)
			.await
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.drop_table(Table::drop().table(PermanentToken::Table).to_owned())
			.await
	}
}
