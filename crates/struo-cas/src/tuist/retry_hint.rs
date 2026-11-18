//! Server retry hints carried as `google.rpc.RetryInfo` in a gRPC status.

use std::time::Duration;

use bazel_remote_apis::google::protobuf::Duration as ProtoDuration;
use bazel_remote_apis::google::rpc::Status as RpcStatus;
use prost::Message;
use tonic::Status;

const RETRY_INFO_TYPE_URL: &str = "type.googleapis.com/google.rpc.RetryInfo";
/// A hint longer than this is treated as this long; the retry budget decides
/// whether waiting that long is still worthwhile.
const MAX_HINT: Duration = Duration::from_secs(30);

#[derive(Clone, PartialEq, Message)]
struct RetryInfo {
    #[prost(message, optional, tag = "1")]
    retry_delay: Option<ProtoDuration>,
}

/// How long the server asked the client to wait before retrying, if it did.
pub(super) fn server_delay(status: &Status) -> Option<Duration> {
    if status.details().is_empty() {
        return None;
    }
    let details = RpcStatus::decode(status.details()).ok()?.details;
    details
        .iter()
        .filter(|detail| detail.type_url == RETRY_INFO_TYPE_URL)
        .find_map(|detail| RetryInfo::decode(detail.value.as_slice()).ok()?.retry_delay)
        .and_then(|delay| {
            let seconds = u64::try_from(delay.seconds).ok()?;
            let nanos = u32::try_from(delay.nanos).ok()?;
            Some(Duration::new(seconds, nanos.min(999_999_999)).min(MAX_HINT))
        })
}

/// A tonic status equivalent to a per-item status from a batch response,
/// keeping its details so a retry hint survives the conversion.
pub(super) fn from_rpc(status: &RpcStatus) -> Status {
    Status::with_details(
        tonic::Code::from_i32(status.code),
        status.message.clone(),
        status.encode_to_vec().into(),
    )
}

#[cfg(test)]
pub(super) fn hinted(code: tonic::Code, delay: Duration) -> Status {
    let retry_info = RetryInfo {
        retry_delay: Some(ProtoDuration {
            seconds: i64::try_from(delay.as_secs()).unwrap(),
            nanos: i32::try_from(delay.subsec_nanos()).unwrap(),
        }),
    };
    let details = RpcStatus {
        code: code as i32,
        message: "slow down".into(),
        details: vec![bazel_remote_apis::google::protobuf::Any {
            type_url: RETRY_INFO_TYPE_URL.into(),
            value: retry_info.encode_to_vec(),
        }],
    };
    Status::with_details(code, "slow down", details.encode_to_vec().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::Code;

    #[test]
    fn retry_info_is_read_from_status_details() {
        let status = hinted(Code::ResourceExhausted, Duration::from_millis(1500));
        assert_eq!(server_delay(&status), Some(Duration::from_millis(1500)));
        assert_eq!(server_delay(&Status::unavailable("no hint")), None);
        assert_eq!(
            server_delay(&Status::with_details(
                Code::Unavailable,
                "junk",
                vec![1, 2, 3].into()
            )),
            None
        );
    }

    #[test]
    fn hints_are_capped_and_survive_batch_status_conversion() {
        let status = hinted(Code::Unavailable, Duration::from_hours(1));
        assert_eq!(server_delay(&status), Some(MAX_HINT));
        let rpc = RpcStatus::decode(status.details()).unwrap();
        assert_eq!(server_delay(&from_rpc(&rpc)), Some(MAX_HINT));
    }
}
