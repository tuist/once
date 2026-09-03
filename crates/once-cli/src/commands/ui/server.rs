use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::{Json, Router};
use futures::stream;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration};

use super::{assets, Publisher, RunSnapshot};

#[derive(Clone)]
pub(super) struct RunStore {
    snapshot: Arc<Mutex<Option<RunSnapshot>>>,
    updates: watch::Sender<Option<RunSnapshot>>,
}

impl RunStore {
    pub(super) fn new() -> Self {
        let (updates, _) = watch::channel(None);
        Self {
            snapshot: Arc::new(Mutex::new(None)),
            updates,
        }
    }

    pub(super) fn replace(&self, run: RunSnapshot) {
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *snapshot = Some(run);
        if self.updates.receiver_count() > 0 {
            self.updates.send_replace(snapshot.clone());
        }
    }

    pub(super) fn update(&self, change: impl FnOnce(&mut RunSnapshot)) {
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(run) = snapshot.as_mut() {
            change(run);
            if self.updates.receiver_count() > 0 {
                self.updates.send_replace(snapshot.clone());
            }
        }
    }

    pub(super) fn latest(&self) -> Option<RunSnapshot> {
        self.snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn subscribe(&self) -> watch::Receiver<Option<RunSnapshot>> {
        self.updates.subscribe()
    }
}

pub struct UiServer {
    store: RunStore,
    address: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl UiServer {
    pub async fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("starting the local Runs server")?;
        let address = listener
            .local_addr()
            .context("reading the local Runs server address")?;
        let store = RunStore::new();
        let app = router(store.clone());
        let (shutdown, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
            {
                tracing::debug!(error = %error, "local Runs server stopped unexpectedly");
            }
        });
        Ok(Self {
            store,
            address,
            shutdown: Some(shutdown),
            task: Some(task),
        })
    }

    #[must_use]
    pub fn publisher(&self) -> Publisher {
        Publisher::from_store(self.store.clone())
    }

    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}/runs/overview", self.address)
    }

    pub async fn write_static_site(&self, workspace: &Path) -> Result<Option<PathBuf>> {
        let Some(run) = self.store.latest() else {
            return Ok(None);
        };
        let directory = workspace.join(".once").join("runs").join(&run.run_id);
        std::fs::create_dir_all(&directory).context("creating the static Runs site")?;
        let report = directory.join("index.html");
        self.store.update(|run| {
            run.static_report_path = Some(report.to_string_lossy().into_owned());
        });
        let run = self
            .store
            .latest()
            .expect("the Runs snapshot exists while writing its static report");
        std::fs::write(&report, static_html(&run)).context("writing the static Runs page")?;
        sleep(Duration::from_millis(250)).await;
        Ok(Some(report))
    }
}

impl Drop for UiServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn router(store: RunStore) -> Router {
    Router::new()
        .route("/api/runs/latest", get(latest))
        .route("/api/runs/events", get(events))
        .route("/assets/app.css", get(styles))
        .route("/assets/app.js", get(script))
        .fallback(get(index))
        .with_state(store)
}

async fn latest(State(store): State<RunStore>) -> Json<Option<RunSnapshot>> {
    Json(store.latest())
}

async fn events(
    State(store): State<RunStore>,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let stream = stream::unfold(store.subscribe(), |mut updates| async move {
        if updates.changed().await.is_err() {
            return None;
        }
        let data = serde_json::to_string(&*updates.borrow()).unwrap_or_else(|error| {
            tracing::debug!(%error, "could not encode a Runs update");
            "null".to_string()
        });
        Some((Ok(Event::default().event("state").data(data)), updates))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn index() -> Html<&'static str> {
    Html(assets::HTML)
}

async fn styles() -> impl IntoResponse {
    ([("content-type", "text/css; charset=utf-8")], assets::CSS)
}

async fn script() -> impl IntoResponse {
    (
        [("content-type", "text/javascript; charset=utf-8")],
        assets::JAVASCRIPT,
    )
}

fn static_html(run: &RunSnapshot) -> String {
    let run = serde_json::to_string(run)
        .expect("serializing an already serializable Runs snapshot should not fail")
        .replace('<', "\\u003c");
    assets::HTML
        .replace(
            "    <link rel=\"stylesheet\" href=\"/assets/app.css\">",
            &format!("    <style>\n{}\n    </style>", assets::CSS),
        )
        .replace(
            "    <script type=\"module\" src=\"/assets/app.js\"></script>",
            &format!(
                "    <script>window.__ONCE_RUN__ = {run};</script>\n    <script type=\"module\">\n{}\n    </script>",
                assets::JAVASCRIPT
            ),
        )
}

#[cfg(test)]
mod tests {
    use super::{static_html, UiServer};
    use crate::commands::ui::{OutputRecord, RunSnapshot};

    #[tokio::test]
    async fn serves_the_static_client_from_the_once_process() {
        let server = UiServer::start().await.unwrap();
        let origin = format!("http://{}", server.address);

        let page = reqwest::get(format!("{origin}/runs/progress"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        let script = reqwest::get(format!("{origin}/assets/app.js"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();

        assert!(page.contains("/assets/app.js"));
        assert!(page.contains("@tuist/noora"));
        assert!(script.contains("/api/runs/events"));
    }

    #[test]
    fn embeds_the_finished_run_in_the_static_page() {
        let page = static_html(&RunSnapshot {
            run_id: "run-123".to_string(),
            action_digest: "digest".to_string(),
            workspace: "workspace".to_string(),
            target: "target".to_string(),
            display_target: "target".to_string(),
            operation: "build".to_string(),
            command: "once build target".to_string(),
            status: "completed".to_string(),
            started_at_ms: 0,
            duration_ms: Some(10),
            cache: Some("miss".to_string()),
            exit_code: Some(0),
            graph: None,
            test_results: None,
            logs: vec![OutputRecord {
                stream: "stdout".to_string(),
                text: "finished".to_string(),
                at_ms: 1,
            }],
            output_truncated: false,
            static_report_path: None,
            output_byte_count: 0,
        });

        assert!(page.contains("window.__ONCE_RUN__"));
        assert!(page.contains("run-123"));
        assert!(page.contains("finished"));
        assert!(page.contains("Run analytics"));
        assert!(page.contains("Cache decision"));
        assert!(page.contains("Resolved targets"));
        assert!(page.contains("Graph filters"));
        assert!(page.contains("Filter graph"));
        assert!(page.contains("document.title"));
        assert!(page.contains("Once Runs"));
        assert!(page.contains("noora-card__section tuist-widget"));
        assert!(page.contains("slot=\"icon\""));
        assert!(page.contains(">Current<"));
        assert!(page.contains("<style>"));
        assert!(!page.contains("/assets/app.js"));
        assert!(!page.contains("/assets/app.css"));
    }
}
