#![allow(clippy::enum_variant_names)]

pub use sea_orm_migration::prelude::*;

mod m20220101_000001_create_users;
mod m20260302_100018_create_session;
mod m20260306_021131_create_item_model;
mod m20260809_041454_create_permanent_tokens;
mod m20260810_031003_create_items;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
	fn migrations() -> Vec<Box<dyn MigrationTrait>> {
		vec![
			Box::new(m20220101_000001_create_users::Migration),
			Box::new(m20260302_100018_create_session::Migration),
			Box::new(m20260306_021131_create_item_model::Migration),
			Box::new(m20260809_041454_create_permanent_tokens::Migration),
			Box::new(m20260810_031003_create_items::Migration),
		]
	}
}
