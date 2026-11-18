use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
pub(crate) struct Discovery {
    #[serde(default)]
    pub events: Vec<String>,
}

// Servers that predate the rename only serve the legacy `once` document, so a
// missing `struo` document falls back to it before reporting no discovery.
const DISCOVERY_PATHS: [&str; 2] = ["/.well-known/struo", "/.well-known/once"];

pub(crate) async fn fetch(base: &str) -> Result<Option<Discovery>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    for path in DISCOVERY_PATHS {
        let response = client.get(format!("{base}{path}")).send().await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            continue;
        }
        return read(response.error_for_status()?).await.map(Some);
    }
    Ok(None)
}

async fn read(mut response: reqwest::Response) -> Result<Discovery> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > 64 * 1024 {
            bail!("Struo discovery document exceeds 64 kibibytes");
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).context("decoding Struo discovery")
}

#[cfg(test)]
mod tests {
    use super::fetch;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn serve(routes: &'static [(&'static str, &'static str)]) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request = vec![0_u8; 4096];
                let read = socket.read(&mut request).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..read]).to_string();
                let body = routes
                    .iter()
                    .find(|(path, _)| request.starts_with(&format!("GET {path} ")))
                    .map(|(_, body)| *body);
                let response = match body {
                    Some(body) => format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    ),
                    None => {
                        "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                            .to_string()
                    }
                };
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        base
    }

    #[tokio::test]
    async fn prefers_the_struo_document_and_falls_back_to_the_legacy_one() {
        let both = serve(&[
            ("/.well-known/struo", r#"{"events":["grpc://struo"]}"#),
            ("/.well-known/once", r#"{"events":["grpc://legacy"]}"#),
        ])
        .await;
        let legacy = serve(&[("/.well-known/once", r#"{"events":["grpc://legacy"]}"#)]).await;
        let neither = serve(&[]).await;

        assert_eq!(
            fetch(&both).await.unwrap().unwrap().events,
            ["grpc://struo"]
        );
        assert_eq!(
            fetch(&legacy).await.unwrap().unwrap().events,
            ["grpc://legacy"]
        );
        assert!(fetch(&neither).await.unwrap().is_none());
    }
}
