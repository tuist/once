use tonic::transport::Uri;

type LinkHandler = Box<dyn Fn(&str) + Send + Sync>;

#[derive(Default)]
pub(crate) struct DashboardLink {
    handler: Option<LinkHandler>,
}

impl DashboardLink {
    pub(crate) fn new(handler: LinkHandler) -> Self {
        Self {
            handler: Some(handler),
        }
    }

    pub(crate) fn accepted(&mut self, server_link: &str) {
        if valid_link(server_link) {
            if let Some(handler) = self.handler.take() {
                handler(server_link);
            }
        }
    }
}

fn valid_link(link: &str) -> bool {
    if link.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let Ok(uri) = link.parse::<Uri>() else {
        return false;
    };
    uri.host().is_some_and(|host| {
        uri.scheme_str() == Some("https")
            || (uri.scheme_str() == Some("http")
                && matches!(host, "localhost" | "127.0.0.1" | "[::1]"))
    }) && uri
        .authority()
        .is_some_and(|authority| !authority.as_str().contains('@'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_terminal_controls_credentials_and_non_web_links() {
        for link in [
            "https://example.com/\nrun",
            "file:///tmp/run",
            "/run/1",
            "https://user:password@example.com/run",
            "http://example.com/run",
        ] {
            assert!(!valid_link(link));
        }
    }

    #[test]
    fn canonical_link_is_reported_once() {
        let links = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = links.clone();
        let mut link = DashboardLink::new(Box::new(move |url| {
            captured.lock().unwrap().push(url.to_string());
        }));
        link.accepted("https://example.com/canonical");
        link.accepted("https://example.com/canonical");
        assert_eq!(*links.lock().unwrap(), ["https://example.com/canonical"]);
    }
}
