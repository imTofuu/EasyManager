use crate::m20220101_000001_create_users::User;
use crate::m20260810_031003_create_items::Item;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(Iden)]
enum Borrow {
	Table,
	BorrowId,
	UserId,
	ItemId,
	CreatedAt,
	Returned,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_table(
				Table::create()
					.table(Borrow::Table)
					.if_not_exists()
					.col(
						ColumnDef::new(Borrow::BorrowId)
							.primary_key()
							.uuid()
							.not_null(),
					)
					.col(ColumnDef::new(Borrow::UserId).uuid().not_null())
					.col(ColumnDef::new(Borrow::ItemId).uuid().not_null())
					.col(
						ColumnDef::new(Borrow::CreatedAt)
							.timestamp_with_time_zone()
							.default(Expr::current_timestamp())
							.not_null(),
					)
					.col(
						ColumnDef::new(Borrow::Returned)
							.boolean()
							.default(false)
							.not_null(),
					)
					.foreign_key(
						ForeignKey::create()
							.name("fk-borrow-userid")
							.from(Borrow::Table, Borrow::UserId)
							.to(User::Table, User::UserId),
					)
					.foreign_key(
						ForeignKey::create()
							.name("fk-borrow-itemid")
							.from(Borrow::Table, Borrow::ItemId)
							.to(Item::Table, Item::ItemId),
					)
					.to_owned(),
			)
			.await
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.drop_table(Table::drop().table(Borrow::Table).to_owned())
			.await
	}
}
