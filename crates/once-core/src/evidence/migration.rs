use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(M20260619000000CreateEvidenceRecords)]
    }
}

#[derive(DeriveMigrationName)]
struct M20260619000000CreateEvidenceRecords;

#[async_trait::async_trait]
impl MigrationTrait for M20260619000000CreateEvidenceRecords {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(EvidenceRecords::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(EvidenceRecords::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(EvidenceRecords::Schema).string().not_null())
                    .col(ColumnDef::new(EvidenceRecords::Kind).string().not_null())
                    .col(
                        ColumnDef::new(EvidenceRecords::SubjectKind)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EvidenceRecords::SubjectId)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(EvidenceRecords::SubjectCapability).string())
                    .col(ColumnDef::new(EvidenceRecords::Status).string().not_null())
                    .col(
                        ColumnDef::new(EvidenceRecords::ActionDigest)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(EvidenceRecords::InputDigest).string())
                    .col(ColumnDef::new(EvidenceRecords::Cache).string().not_null())
                    .col(
                        ColumnDef::new(EvidenceRecords::ExitCode)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(EvidenceRecords::StdoutDigest).string())
                    .col(ColumnDef::new(EvidenceRecords::StderrDigest).string())
                    .col(
                        ColumnDef::new(EvidenceRecords::OutputsJson)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EvidenceRecords::CreatedAtUnixMs)
                            .big_integer()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_evidence_records_subject")
                    .table(EvidenceRecords::Table)
                    .col(EvidenceRecords::SubjectKind)
                    .col(EvidenceRecords::SubjectId)
                    .col(EvidenceRecords::SubjectCapability)
                    .col(EvidenceRecords::CreatedAtUnixMs)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_evidence_records_action")
                    .table(EvidenceRecords::Table)
                    .col(EvidenceRecords::ActionDigest)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(EvidenceRecords::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum EvidenceRecords {
    Table,
    Id,
    Schema,
    Kind,
    SubjectKind,
    SubjectId,
    SubjectCapability,
    Status,
    ActionDigest,
    InputDigest,
    Cache,
    ExitCode,
    StdoutDigest,
    StderrDigest,
    OutputsJson,
    CreatedAtUnixMs,
}
