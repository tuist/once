use sea_orm_migration::prelude::*;

pub(super) struct M20260716000000CreateTestBatchAttempts;

impl MigrationName for M20260716000000CreateTestBatchAttempts {
    fn name(&self) -> &'static str {
        "m20260716_000000_create_test_batch_attempts"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for M20260716000000CreateTestBatchAttempts {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(TestBatchAttempts::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(TestBatchAttempts::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(TestBatchAttempts::Schema)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TestBatchAttempts::PlanId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TestBatchAttempts::BatchId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TestBatchAttempts::Target)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TestBatchAttempts::Attempt)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TestBatchAttempts::Placement)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TestBatchAttempts::Worker)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(TestBatchAttempts::EstimatedDurationMs).big_integer())
                    .col(
                        ColumnDef::new(TestBatchAttempts::StartedAtUnixMs)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TestBatchAttempts::FinishedAtUnixMs)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TestBatchAttempts::DurationMs)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(TestBatchAttempts::Status)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(TestBatchAttempts::ExitCode).integer())
                    .col(ColumnDef::new(TestBatchAttempts::Cache).string())
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_test_batch_attempts_batch")
                    .table(TestBatchAttempts::Table)
                    .col(TestBatchAttempts::BatchId)
                    .col(TestBatchAttempts::StartedAtUnixMs)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_test_batch_attempts_plan")
                    .table(TestBatchAttempts::Table)
                    .col(TestBatchAttempts::PlanId)
                    .col(TestBatchAttempts::StartedAtUnixMs)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(TestBatchAttempts::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum TestBatchAttempts {
    Table,
    Id,
    Schema,
    PlanId,
    BatchId,
    Target,
    Attempt,
    Placement,
    Worker,
    EstimatedDurationMs,
    StartedAtUnixMs,
    FinishedAtUnixMs,
    DurationMs,
    Status,
    ExitCode,
    Cache,
}
