use prost::bytes::Buf;
use prost::encoding::{decode_key, decode_varint, skip_field, DecodeContext, WireType};
use prost::Message;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};

use super::*;

const MAX_STATUS_DETAILS: usize = 128;
const SERVICE: &str = "build.bazel.remote.execution.v2.ContentAddressableStorage";

pub(super) async fn read(
    channel: Channel,
    mut request: Request<reapi::BatchReadBlobsRequest>,
    response_limit: usize,
    timeout: Duration,
) -> std::result::Result<tonic::Response<reapi::BatchReadBlobsResponse>, tonic::Status> {
    let codec = BatchCodec(request.get_ref().digests.len());
    request
        .extensions_mut()
        .insert(tonic::GrpcMethod::new(SERVICE, "BatchReadBlobs"));
    let mut client = tonic::client::Grpc::new(channel).max_decoding_message_size(response_limit);
    tokio::time::timeout(timeout, async move {
        client
            .ready()
            .await
            .map_err(|_| tonic::Status::unavailable("batch read service is not ready"))?;
        client
            .unary(
                request,
                tonic::codegen::http::uri::PathAndQuery::from_static(
                    "/build.bazel.remote.execution.v2.ContentAddressableStorage/BatchReadBlobs",
                ),
                codec,
            )
            .await
    })
    .await
    .unwrap_or_else(|_| {
        Err(tonic::Status::deadline_exceeded(
            "batch read did not complete",
        ))
    })
}

struct BatchCodec(usize);
struct BatchEncoder;
struct BatchDecoder(usize);

impl Codec for BatchCodec {
    type Encode = reapi::BatchReadBlobsRequest;
    type Decode = reapi::BatchReadBlobsResponse;
    type Encoder = BatchEncoder;
    type Decoder = BatchDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        BatchEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        BatchDecoder(self.0)
    }
}

impl Encoder for BatchEncoder {
    type Item = reapi::BatchReadBlobsRequest;
    type Error = tonic::Status;

    fn encode(
        &mut self,
        item: Self::Item,
        buffer: &mut EncodeBuf<'_>,
    ) -> std::result::Result<(), Self::Error> {
        item.encode(buffer)
            .map_err(|_| tonic::Status::internal("encoding batch read request"))
    }
}

impl Decoder for BatchDecoder {
    type Item = reapi::BatchReadBlobsResponse;
    type Error = tonic::Status;

    fn decode(
        &mut self,
        buffer: &mut DecodeBuf<'_>,
    ) -> std::result::Result<Option<Self::Item>, Self::Error> {
        decode(buffer, self.0).map(Some)
    }
}

fn decode(
    buffer: &mut impl Buf,
    expected: usize,
) -> std::result::Result<reapi::BatchReadBlobsResponse, tonic::Status> {
    let mut counts = Counts {
        responses: 0,
        details: 0,
        expected,
    };
    // A wire-byte limit alone does not bound allocations for repeated empty messages.
    validate(buffer.chunk(), Shape::Batch, &mut counts)?;
    reapi::BatchReadBlobsResponse::decode(buffer).map_err(|_| malformed())
}

struct Counts {
    responses: usize,
    details: usize,
    expected: usize,
}

#[derive(Clone, Copy)]
enum Shape {
    Batch,
    Blob,
    Status,
}

fn validate(
    mut bytes: &[u8],
    shape: Shape,
    counts: &mut Counts,
) -> std::result::Result<(), tonic::Status> {
    while bytes.has_remaining() {
        let (tag, wire) = decode_key(&mut bytes).map_err(|_| malformed())?;
        match (shape, tag) {
            (Shape::Batch, 1) => {
                counts.responses += 1;
                if counts.responses > counts.expected {
                    return Err(malformed());
                }
                validate(message(&mut bytes, wire)?, Shape::Blob, counts)?;
            }
            (Shape::Blob, 3) => validate(message(&mut bytes, wire)?, Shape::Status, counts)?,
            (Shape::Status, 3) => {
                counts.details += 1;
                if counts.details > MAX_STATUS_DETAILS {
                    return Err(malformed());
                }
                skip_field(wire, tag, &mut bytes, DecodeContext::default())
                    .map_err(|_| malformed())?;
            }
            _ => skip_field(wire, tag, &mut bytes, DecodeContext::default())
                .map_err(|_| malformed())?,
        }
    }
    Ok(())
}

fn message<'a>(
    bytes: &mut &'a [u8],
    wire: WireType,
) -> std::result::Result<&'a [u8], tonic::Status> {
    if wire != WireType::LengthDelimited {
        return Err(malformed());
    }
    let length =
        usize::try_from(decode_varint(bytes).map_err(|_| malformed())?).map_err(|_| malformed())?;
    let (message, rest) = bytes.split_at_checked(length).ok_or_else(malformed)?;
    *bytes = rest;
    Ok(message)
}

fn malformed() -> tonic::Status {
    tonic::Status::invalid_argument(
        "batch read response exceeds its structural bounds or is malformed",
    )
}

#[cfg(test)]
mod tests;
