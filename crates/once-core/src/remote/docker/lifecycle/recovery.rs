use super::{lifecycle, retained, Client, Provider};

pub(super) fn failed_delete(client: &Client, id: &str) {
    let scope = client.scope();
    let keys = retained()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter(|((owner, _), reservation)| {
            owner == &scope && reservation.id.as_deref() == Some(id)
        })
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for key in keys {
        start(client.clone(), key);
    }
}

pub(super) fn start(client: Client, key: (String, String)) {
    {
        let mut reservations = retained()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(reservation) = reservations.get_mut(&key) else {
            return;
        };
        if reservation.recovering {
            return;
        }
        reservation.recovering = true;
    }
    // Reconciliation cannot queue behind admission held by the orphan itself.
    let recovery: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
        Box::pin(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let ownership = retained()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&key)
                    .map(|reservation| (reservation.journal.clone(), reservation.lease.clone()));
                let Some((journal, _lease)) = ownership else {
                    break;
                };
                lifecycle::reconcile(&journal, &client, lifecycle::Budget::default()).await;
            }
        });
    tokio::spawn(recovery);
}
