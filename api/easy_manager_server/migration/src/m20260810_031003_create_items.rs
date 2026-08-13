use sea_orm_migration::prelude::*;

use crate::m20260306_021131_create_item_model::ItemModel;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(Iden)]
pub enum Item {
	Table,
	ItemId,
	Name,
	ItemModelId,
	PermissionLevel,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_table(
				Table::create()
					.table(Item::Table)
					.if_not_exists()
					.col(ColumnDef::new(Item::ItemId).primary_key().uuid().not_null())
					.col(ColumnDef::new(Item::Name).string().not_null())
					.col(ColumnDef::new(Item::ItemModelId).uuid().not_null())
					.col(ColumnDef::new(Item::PermissionLevel).string())
					.foreign_key(
						ForeignKey::create()
							.name("fk-itemid-itemmodelid")
							.from(Item::Table, Item::ItemModelId)
							.to(ItemModel::Table, ItemModel::ItemModelId),
					)
					.to_owned(),
			)
			.await
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.drop_table(Table::drop().table(Item::Table).to_owned())
			.await
	}
}
