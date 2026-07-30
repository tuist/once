use sea_orm_migration::prelude::*;

pub struct M20260729000000AddInputFingerprint;

impl MigrationName for M20260729000000AddInputFingerprint {
    fn name(&self) -> &'static str {
        "m20260729_000000_add_input_fingerprint"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for M20260729000000AddInputFingerprint {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(EvidenceRecords::Table)
                    .add_column(ColumnDef::new(EvidenceRecords::InputFingerprintJson).text())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(EvidenceRecords::Table)
                    .drop_column(EvidenceRecords::InputFingerprintJson)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum EvidenceRecords {
    Table,
    InputFingerprintJson,
}
