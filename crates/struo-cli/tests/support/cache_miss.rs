use bazel_remote_apis::build::bazel::remote::execution::v2::{
    self as reapi,
    action_cache_server::{ActionCache, ActionCacheServer},
};
use tonic::{Request, Response, Status};

#[derive(Clone)]
struct Misses;

#[tonic::async_trait]
impl ActionCache for Misses {
    async fn get_action_result(
        &self,
        _: Request<reapi::GetActionResultRequest>,
    ) -> Result<Response<reapi::ActionResult>, Status> {
        Err(Status::not_found("no retained action result"))
    }
    async fn update_action_result(
        &self,
        request: Request<reapi::UpdateActionResultRequest>,
    ) -> Result<Response<reapi::ActionResult>, Status> {
        Ok(Response::new(
            request.into_inner().action_result.unwrap_or_default(),
        ))
    }
}

pub async fn endpoint() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let incoming = futures::stream::unfold(listener, |listener| async {
        Some((listener.accept().await.map(|(socket, _)| socket), listener))
    });
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(ActionCacheServer::new(Misses))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    format!("http://{address}")
}
