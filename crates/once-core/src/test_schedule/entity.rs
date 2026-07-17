use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "test_batch_attempts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub schema: String,
    pub plan_id: String,
    pub batch_id: String,
    pub target: String,
    pub attempt: i32,
    pub placement: String,
    pub worker: String,
    pub estimated_duration_ms: Option<i64>,
    pub started_at_unix_ms: i64,
    pub finished_at_unix_ms: i64,
    pub duration_ms: i64,
    pub status: String,
    pub exit_code: Option<i32>,
    pub cache: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
