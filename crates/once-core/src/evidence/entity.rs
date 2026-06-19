use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "evidence_records")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub schema: String,
    pub kind: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub subject_capability: Option<String>,
    pub status: String,
    pub action_digest: String,
    pub input_digest: Option<String>,
    pub cache: String,
    pub exit_code: i32,
    pub stdout_digest: Option<String>,
    pub stderr_digest: Option<String>,
    pub outputs_json: String,
    pub created_at_unix_ms: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
