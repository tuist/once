use bazel_remote_apis::build::bazel::remote::execution::v2::{
    self as reapi,
    action_cache_server::{ActionCache, ActionCacheServer},
};
use tonic::{Code, Request, Response, Status};

#[derive(Clone)]
struct FailingCache(Code);

#[tonic::async_trait]
impl ActionCache for FailingCache {
    async fn get_action_result(
        &self,
        _: Request<reapi::GetActionResultRequest>,
    ) -> Result<Response<reapi::ActionResult>, Status> {
        Err(Status::new(self.0, "test cache failure"))
    }

    async fn update_action_result(
        &self,
        _: Request<reapi::UpdateActionResultRequest>,
    ) -> Result<Response<reapi::ActionResult>, Status> {
        Err(Status::new(self.0, "test cache failure"))
    }
}

pub async fn endpoint(code: Code) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let incoming = futures::stream::unfold(listener, |listener| async {
        Some((listener.accept().await.map(|(socket, _)| socket), listener))
    });
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(ActionCacheServer::new(FailingCache(code)))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    format!("http://{address}")
}
