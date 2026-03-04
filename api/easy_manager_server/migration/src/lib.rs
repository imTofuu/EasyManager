#![allow(clippy::enum_variant_names)]

pub use sea_orm_migration::prelude::*;

mod m20220101_000001_create_users;
mod m20260302_100018_create_session;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
	fn migrations() -> Vec<Box<dyn MigrationTrait>> {
		vec![
			Box::new(m20220101_000001_create_users::Migration),
			Box::new(m20260302_100018_create_session::Migration),
		]
	}
}
