use std::time::{Duration, Instant};

use super::journal::{self, Claim, Entry, Journal, Record};
use super::{blocking, delete_with_retries, Lookup, Provider};

const MAX_BACKOFF_SECS: u64 = 60 * 60;
const BASE_BACKOFF_SECS: u64 = 30;

/// Limits a reconciliation pass so it never dominates the action that runs
/// it: entries scanned, records acted on, and total elapsed time.
#[derive(Clone, Copy, Debug)]
pub(in crate::remote) struct Budget {
    pub scan: usize,
    pub records: usize,
    pub duration: Duration,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            scan: journal::MAX_ENTRIES,
            records: 16,
            duration: Duration::from_secs(20),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::remote) struct Report {
    /// Sandboxes whose deletion the provider confirmed.
    pub deleted: usize,
    /// Records whose marker no sandbox carries any more.
    pub absent: usize,
    /// Records kept for a later pass.
    pub retained: usize,
    /// Records held by a live owner.
    pub active: usize,
    /// Records for another provider or endpoint.
    pub foreign: usize,
    pub quarantined: usize,
}

/// Repairs records left by earlier operations of this provider scope.
///
/// Records whose owner still holds the lock are skipped. Every other record
/// is looked up by its marker: verified matches are journaled and deleted; a
/// complete listing without a match is accepted as absence only after the
/// record's settle time, so a create that is still becoming visible is not
/// forgotten. Anything else is kept.
/// Failures never fail the caller; they are logged and retried later.
pub(in crate::remote) async fn reconcile<P: Provider>(
    journal: &Journal,
    provider: &P,
    budget: Budget,
) -> Report {
    let deadline = Instant::now() + budget.duration;
    let mut report = Report::default();
    let scanner = journal.clone();
    let nonces = match blocking(move || scanner.scan(budget.scan)).await {
        Ok(nonces) => nonces,
        Err(error) => {
            tracing::warn!(provider = P::NAME, %error, "failed to scan remote lifecycle journal");
            return report;
        }
    };
    let scope = provider.scope();
    let mut acted = 0;
    for nonce in nonces {
        if acted >= budget.records || Instant::now() >= deadline {
            break;
        }
        let claimer = journal.clone();
        let claim = match blocking(move || claimer.claim(&nonce)).await {
            Ok(claim) => claim,
            Err(error) => {
                tracing::warn!(provider = P::NAME, %error, "failed to claim remote lifecycle record");
                continue;
            }
        };
        let entry = match claim {
            Claim::Entry(entry) => *entry,
            Claim::Active => {
                report.active += 1;
                continue;
            }
            Claim::Quarantined => {
                report.quarantined += 1;
                continue;
            }
            Claim::Missing => continue,
        };
        let record = entry.record();
        if record.provider != P::NAME || record.scope != scope {
            report.foreign += 1;
            continue;
        }
        if record.next_check_at > journal::now() {
            report.retained += 1;
            continue;
        }
        acted += 1;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if tokio::time::timeout(remaining, repair(provider, entry, &mut report))
            .await
            .is_err()
        {
            report.retained += 1;
            tracing::debug!(provider = P::NAME, "reconciliation budget elapsed");
            break;
        }
    }
    if report != Report::default() {
        tracing::info!(
            provider = P::NAME,
            deleted = report.deleted,
            absent = report.absent,
            retained = report.retained,
            active = report.active,
            foreign = report.foreign,
            quarantined = report.quarantined,
            "reconciled remote sandbox lifecycle journal"
        );
    }
    report
}

/// Every deletion here is preceded by an authenticated lookup of the
/// record's marker, including for records that already name an id: a
/// journaled id may since have been deleted and reused, or the record may have
/// been copied or edited. Only ids the provider currently reports with the
/// exact marker are deleted; journaled ids it does not report are left alone.
async fn repair<P: Provider>(provider: &P, mut entry: Entry, report: &mut Report) {
    let nonce = entry.nonce().to_string();
    let ids = match provider.find(&nonce).await {
        Ok(Lookup::Found(ids)) => ids,
        Ok(Lookup::Absent) if settled(entry.record(), provider) => {
            tracing::debug!(provider = P::NAME, %nonce, phase = ?entry.record().phase, "no sandbox carries the marker");
            finish(entry, report, |report| report.absent += 1).await;
            return;
        }
        Ok(Lookup::Absent | Lookup::Inconclusive) => {
            defer(entry, report).await;
            return;
        }
        Err(error) => {
            tracing::debug!(provider = P::NAME, %nonce, %error, "failed to look up a lifecycle marker");
            defer(entry, report).await;
            return;
        }
    };
    let verified = ids.clone();
    match blocking(move || entry.mark_created(&verified).map(|()| entry)).await {
        Ok(updated) => entry = updated,
        Err(error) => {
            tracing::warn!(provider = P::NAME, %nonce, %error, "failed to journal a verified sandbox");
            report.retained += 1;
            return;
        }
    }
    let mut deleted = 0;
    for id in &ids {
        match delete_with_retries(provider, id).await {
            Ok(()) => deleted += 1,
            Err(error) => {
                tracing::warn!(provider = P::NAME, sandbox = %id, %error, "failed to delete a journaled sandbox");
            }
        }
    }
    if deleted == ids.len() {
        finish(entry, report, |report| report.deleted += deleted).await;
    } else {
        report.deleted += deleted;
        defer(entry, report).await;
    }
}

/// A complete listing without the marker proves absence only after the
/// record settles, and only under the credential that created it. Another
/// credential may see a different account through the same endpoint, so its
/// empty listing proves nothing however much time passes: such records are
/// kept until the original credential returns, a lookup finds the marker, or
/// someone removes them by hand. They keep counting toward the journal's
/// capacity, so enough of them make new creates fail closed.
fn settled<P: Provider>(record: &Record, provider: &P) -> bool {
    record.credential == provider.credential() && record.settles_at <= journal::now()
}

async fn finish(entry: Entry, report: &mut Report, count: impl FnOnce(&mut Report)) {
    match blocking(move || entry.complete()).await {
        Ok(()) => count(report),
        Err(error) => {
            tracing::warn!(%error, "failed to remove a reconciled lifecycle record");
            report.retained += 1;
        }
    }
}

async fn defer(mut entry: Entry, report: &mut Report) {
    let attempts = entry.record().attempts.min(16);
    let backoff = BASE_BACKOFF_SECS
        .saturating_mul(1 << attempts)
        .min(MAX_BACKOFF_SECS);
    if let Err(error) = blocking(move || entry.defer(Duration::from_secs(backoff))).await {
        tracing::warn!(%error, "failed to update a lifecycle record");
    }
    report.retained += 1;
}
