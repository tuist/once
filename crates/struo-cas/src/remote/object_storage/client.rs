use object_store::client::{
    HttpClient, HttpConnector, HttpError, HttpErrorKind, HttpRequest, HttpResponse, HttpService,
    ReqwestConnector,
};
use object_store::ClientOptions;
use reqwest::{Method, StatusCode};

#[derive(Debug)]
pub(super) struct CredentialConnector;

impl HttpConnector for CredentialConnector {
    fn connect(&self, options: &ClientOptions) -> object_store::Result<HttpClient> {
        Ok(HttpClient::new(CredentialService(
            ReqwestConnector::default().connect(options)?,
        )))
    }
}

#[derive(Debug)]
pub(super) struct CredentialService(pub(super) HttpClient);

#[derive(Debug, thiserror::Error)]
#[error("credential or permission request refused (HTTP {0})")]
pub(super) struct Refused(pub(super) u16);

#[async_trait::async_trait]
impl HttpService for CredentialService {
    async fn call(&self, request: HttpRequest) -> std::result::Result<HttpResponse, HttpError> {
        let token_exchange = request.method() == Method::POST
            && request
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| {
                    value.split(';').next().is_some_and(|value| {
                        value
                            .trim()
                            .eq_ignore_ascii_case("application/x-www-form-urlencoded")
                    })
                });
        let response = self.0.execute(request).await?;
        let status = response.status();
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
            || (token_exchange && status == StatusCode::BAD_REQUEST)
        {
            // Credential errors are otherwise hidden inside private provider
            // types. Preserve the refusal without retaining a response body.
            return Err(HttpError::new(
                HttpErrorKind::Unknown,
                Refused(status.as_u16()),
            ));
        }
        Ok(response)
    }
}

pub(super) fn refused(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut source = Some(error);
    while let Some(error) = source {
        if error.is::<Refused>() {
            return true;
        }
        source = error.source();
    }
    false
}
