use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(Iden)]
enum ItemModel {
	Table,
	ItemModelId,
	Name,
	Description,
	PermissionLevel
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_table(
				Table::create()
					.table(ItemModel::Table)
					.if_not_exists()
					.col(
						ColumnDef::new(ItemModel::ItemModelId)
							.uuid()
							.primary_key()
							.not_null()
					)
					.col(ColumnDef::new(ItemModel::Name).string().not_null())
					.col(ColumnDef::new(ItemModel::Description).string())
					.col(
						ColumnDef::new(ItemModel::PermissionLevel)
							.string()
							.not_null()
					)
					.to_owned()
			)
			.await
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.drop_table(Table::drop().table(ItemModel::Table).to_owned())
			.await
	}
}
