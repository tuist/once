use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::process::{error, Client};
use crate::remote::lifecycle::{self, HostedSandbox, Lookup, Provider};
use crate::resources::ResourcePermit;
use crate::{ExecutionPermit, ResourceRequest, Result};

pub(super) struct Container(pub String);

const SETTLE: Duration = Duration::from_mins(2);

mod recovery;

struct Reservation {
    lease: ExecutionPermit,
    _worker: ResourcePermit,
    journal: lifecycle::Journal,
    credential: String,
    recovering: bool,
    id: Option<String>,
    settles_at: Instant,
}

fn retained() -> &'static Mutex<BTreeMap<(String, String), Reservation>> {
    static RETAINED: OnceLock<Mutex<BTreeMap<(String, String), Reservation>>> = OnceLock::new();
    RETAINED.get_or_init(Mutex::default)
}

impl HostedSandbox for Container {
    fn id(&self) -> &str {
        &self.0
    }
}

impl Provider for Client {
    const NAME: &'static str = "docker";

    fn scope(&self) -> String {
        lifecycle::scope("docker", self.daemon())
    }

    fn credential(&self) -> String {
        lifecycle::credential("docker", self.credential())
    }

    async fn find(&self, nonce: &str) -> Result<Lookup> {
        self.verify_daemon().await?;
        let bytes = self
            .control(&[
                "ps".into(),
                "--all".into(),
                "--no-trunc".into(),
                "--filter".into(),
                format!("label={}={nonce}", lifecycle::MARKER_KEY),
                "--format".into(),
                format!("{{{{.ID}}}} {{{{.Label \"{}\"}}}}", lifecycle::MARKER_KEY),
            ])
            .await?;
        self.verify_daemon().await?;
        let text = std::str::from_utf8(&bytes).map_err(|_| error("invalid container listing"))?;
        let mut entries = Vec::new();
        for line in text.lines() {
            let Some((id, marker)) = line.split_once(' ') else {
                return Ok(Lookup::Inconclusive);
            };
            if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Ok(Lookup::Inconclusive);
            }
            entries.push((id, Some(marker), true));
        }
        let found = lifecycle::classify(entries, nonce, false);
        let mut reservations = retained()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = (self.scope(), nonce.to_string());
        match &found {
            Lookup::Absent
                if reservations.get(&key).is_some_and(|reservation| {
                    reservation.credential != Provider::credential(self)
                }) =>
            {
                return Ok(Lookup::Inconclusive);
            }
            Lookup::Absent
                if reservations.get(&key).is_some_and(|reservation| {
                    reservation.id.is_some() || Instant::now() >= reservation.settles_at
                }) =>
            {
                reservations.remove(&key);
            }
            Lookup::Absent if reservations.contains_key(&key) => {
                return Ok(Lookup::Inconclusive);
            }
            Lookup::Found(ids) if ids.len() == 1 => {
                if let Some(reservation) = reservations.get_mut(&key) {
                    reservation.id = Some(ids[0].clone());
                }
            }
            _ => {}
        }
        Ok(found)
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let deleted = async {
            self.verify_daemon().await?;
            self.control(&["rm".into(), "--force".into(), "--volumes".into(), id.into()])
                .await?;
            self.verify_daemon().await
        }
        .await;
        if let Err(error) = deleted {
            recovery::failed_delete(self, id);
            return Err(error);
        }
        let scope = self.scope();
        retained()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(owner, _), reservation| {
                owner != &scope || reservation.id.as_deref() != Some(id)
            });
        Ok(())
    }
}

pub(super) async fn create(
    client: Client,
    journal: lifecycle::Journal,
    image: String,
    lease: ExecutionPermit,
    worker: ResourcePermit,
    timeout_ms: Option<u64>,
) -> Result<(Container, lifecycle::Cleanup<Client>)> {
    client.verify_daemon().await?;
    let request = worker.request().clone();
    let retained_lease = lease.clone();
    let retained_journal = journal.clone();
    lifecycle::create_owned(
        client,
        Some(journal),
        SETTLE,
        Some(lease),
        move |client, marker| async move {
            let marker = marker.expect("Docker containers are journaled");
            let name = format!("once-{}", marker.nonce);
            let key = (client.scope(), marker.nonce.clone());
            retained()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(
                    key.clone(),
                    Reservation {
                        lease: retained_lease,
                        _worker: worker,
                        journal: retained_journal,
                        credential: Provider::credential(&client),
                        recovering: false,
                        id: None,
                        settles_at: Instant::now() + SETTLE,
                    },
                );
            let args = create_args(&name, &marker.nonce, &image, &request, timeout_ms);
            // Keep creation alive independently of a dropped action. A failed
            // create can still have allocated the named container.
            let result = client.control(&args).await.and_then(|bytes| {
                let id = std::str::from_utf8(&bytes)
                    .map_err(|_| error("invalid Docker container ID"))?
                    .trim();
                if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(error("invalid Docker container ID"));
                }
                if let Some(reservation) = retained()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get_mut(&key)
                {
                    reservation.id = Some(id.into());
                }
                Ok(Container(id.into()))
            });
            if result.is_err() {
                recovery::start(client, key);
            }
            result
        },
    )
    .await
}

fn create_args(
    name: &str,
    nonce: &str,
    image: &str,
    request: &ResourceRequest,
    timeout_ms: Option<u64>,
) -> Vec<String> {
    let lifetime = timeout_ms.map_or(86_400, |ms| ms.div_ceil(1000).saturating_add(120));
    vec![
        "create".into(),
        "--pull=never".into(),
        "--name".into(),
        name.into(),
        "--label".into(),
        format!("{}={nonce}", lifecycle::MARKER_KEY),
        "--cpus".into(),
        request.cpu_slots.to_string(),
        "--memory".into(),
        request.memory_bytes.to_string(),
        "--memory-swap".into(),
        request.memory_bytes.to_string(),
        "--user".into(),
        "0".into(),
        "--workdir".into(),
        "/".into(),
        "--entrypoint".into(),
        "sleep".into(),
        image.into(),
        lifetime.to_string(),
    ]
}

#[cfg(test)]
#[path = "tests/lifecycle.rs"]
mod tests;
