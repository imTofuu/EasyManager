use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(Iden)]
pub enum User {
	Table,
	UserId,
	Email,
	Username,
	Password,
	PermissionLevel,
	CreatedAt
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_table(
				Table::create()
					.table(User::Table)
					.if_not_exists()
					.col(ColumnDef::new(User::UserId).uuid().primary_key().not_null())
					.col(
						ColumnDef::new(User::Username)
							.string()
							.unique_key()
							.not_null()
					)
					.col(ColumnDef::new(User::Email).string().unique_key().not_null())
					.col(ColumnDef::new(User::Password).string().not_null())
					.col(ColumnDef::new(User::PermissionLevel).string().not_null())
					.col(
						ColumnDef::new(User::CreatedAt)
							.timestamp_with_time_zone()
							.default(Expr::current_timestamp())
							.not_null()
					)
					.to_owned()
			)
			.await
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.drop_table(Table::drop().table(User::Table).to_owned())
			.await
	}
}
