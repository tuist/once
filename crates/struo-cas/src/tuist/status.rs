//! Classification of remote failures into [`ErrorKind`]s.

use bazel_remote_apis::google::rpc::Status as RpcStatus;
use reqwest::StatusCode;
use tonic::{Code, Status};

use super::PROVIDER_NAME;
use crate::{Error, ErrorKind};

pub(super) fn remote(
    operation: &'static str,
    kind: ErrorKind,
    message: impl Into<String>,
) -> Error {
    Error::remote(PROVIDER_NAME, operation, kind, message)
}

pub(super) fn grpc_error(operation: &'static str, status: &Status) -> Error {
    remote(
        operation,
        grpc_kind(status.code()),
        grpc_status_message(status),
    )
}

/// Per-blob statuses inside batch responses use the gRPC code space.
pub(super) fn rpc_status_error(operation: &'static str, status: &RpcStatus) -> Error {
    remote(
        operation,
        grpc_kind(Code::from_i32(status.code)),
        rpc_status_message(status),
    )
}

pub(super) fn grpc_kind(code: Code) -> ErrorKind {
    match code {
        Code::NotFound => ErrorKind::NotFound,
        Code::Unavailable
        | Code::DeadlineExceeded
        | Code::ResourceExhausted
        | Code::Aborted
        | Code::Cancelled
        | Code::Internal
        | Code::Unknown => ErrorKind::Unavailable,
        Code::Unauthenticated | Code::PermissionDenied => ErrorKind::Refused,
        Code::DataLoss => ErrorKind::Corrupt,
        Code::Ok
        | Code::InvalidArgument
        | Code::AlreadyExists
        | Code::FailedPrecondition
        | Code::OutOfRange
        | Code::Unimplemented => ErrorKind::Protocol,
    }
}

pub(super) fn http_kind(status: StatusCode) -> ErrorKind {
    if status.is_server_error()
        || status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::REQUEST_TIMEOUT
    {
        ErrorKind::Unavailable
    } else if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        ErrorKind::Refused
    } else if status == StatusCode::NOT_FOUND {
        // The account or route does not exist. That is a configuration
        // problem, not evidence that cached content is absent.
        ErrorKind::Misconfigured
    } else {
        ErrorKind::Protocol
    }
}

/// A gRPC failure described by its code and by what this client observed.
/// The message text is never included: a server writes it, and it can echo
/// credentials.
pub(super) fn grpc_status_message(status: &Status) -> String {
    let code = status.code();
    if super::stall::is_stalled(status) {
        format!("gRPC {code}: the transfer made no progress within the stall timeout")
    } else if super::retry::timed_out(status) {
        format!("gRPC {code}: the request timed out")
    } else if std::error::Error::source(status).is_some() {
        format!("gRPC {code}: the connection failed")
    } else {
        format!("gRPC {code}")
    }
}

/// A per-item batch status described by its code alone.
pub(super) fn rpc_status_message(status: &RpcStatus) -> String {
    format!("REAPI status {}", status.code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outages_refusals_and_protocol_errors_are_never_absence() {
        for (code, kind) in [
            (Code::Unavailable, ErrorKind::Unavailable),
            (Code::DeadlineExceeded, ErrorKind::Unavailable),
            (Code::ResourceExhausted, ErrorKind::Unavailable),
            (Code::Internal, ErrorKind::Unavailable),
            (Code::Unauthenticated, ErrorKind::Refused),
            (Code::PermissionDenied, ErrorKind::Refused),
            (Code::InvalidArgument, ErrorKind::Protocol),
            (Code::Unimplemented, ErrorKind::Protocol),
            (Code::DataLoss, ErrorKind::Corrupt),
            (Code::NotFound, ErrorKind::NotFound),
        ] {
            let error = grpc_error("get action result", &Status::new(code, "injected"));
            assert_eq!(error.kind(), kind, "{code:?}");
        }
    }

    #[test]
    fn http_statuses_are_classified() {
        assert_eq!(
            http_kind(StatusCode::SERVICE_UNAVAILABLE),
            ErrorKind::Unavailable
        );
        assert_eq!(
            http_kind(StatusCode::TOO_MANY_REQUESTS),
            ErrorKind::Unavailable
        );
        assert_eq!(http_kind(StatusCode::UNAUTHORIZED), ErrorKind::Refused);
        assert_eq!(http_kind(StatusCode::FORBIDDEN), ErrorKind::Refused);
        assert_eq!(http_kind(StatusCode::NOT_FOUND), ErrorKind::Misconfigured);
        assert_eq!(http_kind(StatusCode::BAD_REQUEST), ErrorKind::Protocol);
    }

    #[test]
    fn grpc_error_includes_the_code_but_not_the_server_message() {
        let error = grpc_error("get blob", &Status::unavailable("token=abc123"));
        let message = error.to_string();
        assert!(message.to_lowercase().contains("unavailable"));
        assert!(!message.contains("abc123"));
    }
}
