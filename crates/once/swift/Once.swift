import Foundation
import COnce

public enum OnceError: Error, Equatable {
    case invalidUTF8
    case api(String)
}

public struct OnceCache: Sendable {
    private let localCacheRoot: String?

    public init() {
        self.localCacheRoot = nil
    }

    public init(localCacheRoot: String) {
        self.localCacheRoot = localCacheRoot
    }

    public var version: String {
        String(cString: once_version())
    }

    public func digest(bytes: Data) throws -> String {
        try bytes.withUnsafeBytes { buffer in
            let pointer = buffer.bindMemory(to: UInt8.self).baseAddress
            return try decodeStringResponse(
                once_digest_bytes(pointer, buffer.count)
            )
        }
    }

    public func actionDigest(actionJSON: String) throws -> String {
        try decodeStringResponse(once_action_digest_json(actionJSON))
    }

    @discardableResult
    public func putBlob(_ bytes: Data) throws -> String {
        let request = BlobPutRequest(localCacheRoot: localCacheRoot, bytes: Array(bytes))
        return try decodeRequest(request, with: once_cache_put_blob_json, as: String.self)
    }

    public func getBlob(_ digest: String) throws -> Data {
        let request = DigestRequest(localCacheRoot: localCacheRoot, digest: digest)
        let response = try decodeRequest(request, with: once_cache_get_blob_json, as: BlobResponse.self)
        return Data(response.bytes)
    }

    public func hasBlob(_ digest: String) throws -> Bool {
        let request = DigestRequest(localCacheRoot: localCacheRoot, digest: digest)
        return try decodeRequest(request, with: once_cache_has_blob_json, as: Bool.self)
    }

    @discardableResult
    public func putActionResult(_ result: OnceActionResult, for actionDigest: String) throws -> Bool {
        let request = ActionResultPutRequest(
            localCacheRoot: localCacheRoot,
            actionDigest: actionDigest,
            result: result
        )
        return try decodeRequest(request, with: once_cache_put_action_result_json, as: Bool.self)
    }

    public func getActionResult(_ actionDigest: String) throws -> OnceActionResult? {
        let request = ActionDigestRequest(localCacheRoot: localCacheRoot, actionDigest: actionDigest)
        return try decodeRequest(
            request,
            with: once_cache_get_action_result_json,
            as: OnceActionResult?.self
        )
    }

    @discardableResult
    public func forgetAction(_ actionDigest: String) throws -> Bool {
        let request = ActionDigestRequest(localCacheRoot: localCacheRoot, actionDigest: actionDigest)
        return try decodeRequest(request, with: once_cache_forget_action_json, as: Bool.self)
    }

    public func stats() throws -> OnceCacheStats {
        let request = CacheRootRequest(localCacheRoot: localCacheRoot)
        return try decodeRequest(request, with: once_cache_stats_json, as: OnceCacheStats.self)
    }
}

public struct OnceActionResult: Codable, Sendable, Equatable {
    public var exitCode: Int32
    public var stdout: String?
    public var stderr: String?
    public var outputs: [String: String]

    public init(
        exitCode: Int32,
        stdout: String? = nil,
        stderr: String? = nil,
        outputs: [String: String] = [:]
    ) {
        self.exitCode = exitCode
        self.stdout = stdout
        self.stderr = stderr
        self.outputs = outputs
    }
}

public struct OnceCacheStats: Decodable, Sendable, Equatable {
    public let blobCount: UInt64
    public let blobBytes: UInt64
    public let actionCount: UInt64
    public let actionBytes: UInt64
}

private typealias CJSONFunction = @convention(c) (UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>?

private struct CacheRootRequest: Encodable {
    let localCacheRoot: String?
}

private struct BlobPutRequest: Encodable {
    let localCacheRoot: String?
    let bytes: [UInt8]
}

private struct BlobResponse: Decodable {
    let bytes: [UInt8]
}

private struct DigestRequest: Encodable {
    let localCacheRoot: String?
    let digest: String
}

private struct ActionResultPutRequest: Encodable {
    let localCacheRoot: String?
    let actionDigest: String
    let result: OnceActionResult
}

private struct ActionDigestRequest: Encodable {
    let localCacheRoot: String?
    let actionDigest: String
}

private enum OnceResponse<Value: Decodable>: Decodable {
    case ok(Value)
    case error(String)

    private enum CodingKeys: String, CodingKey {
        case status
        case value
        case message
    }

    private enum Status: String, Decodable {
        case ok
        case error
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(Status.self, forKey: .status) {
        case .ok:
            self = .ok(try container.decode(Value.self, forKey: .value))
        case .error:
            self = .error(try container.decode(String.self, forKey: .message))
        }
    }
}

private extension JSONEncoder {
    static var once: JSONEncoder {
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        return encoder
    }
}

private extension JSONDecoder {
    static var once: JSONDecoder {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }
}

private func decodeRequest<Request: Encodable, Value: Decodable>(
    _ request: Request,
    with function: CJSONFunction,
    as type: Value.Type
) throws -> Value {
    let json = try JSONEncoder.once.encode(request)
    guard let requestJSON = String(data: json, encoding: .utf8) else {
        throw OnceError.invalidUTF8
    }
    return try requestJSON.withCString { pointer in
        try decodeResponse(function(pointer), as: Value.self)
    }
}

private func decodeStringResponse(_ raw: UnsafeMutablePointer<CChar>?) throws -> String {
    try decodeResponse(raw, as: String.self)
}

private func decodeResponse<Value: Decodable>(
    _ raw: UnsafeMutablePointer<CChar>?,
    as type: Value.Type
) throws -> Value {
    guard let raw else {
        throw OnceError.invalidUTF8
    }
    defer { once_string_free(raw) }
    let json = String(cString: raw)
    let data = Data(json.utf8)
    switch try JSONDecoder.once.decode(OnceResponse<Value>.self, from: data) {
    case let .ok(value):
        return value
    case let .error(message):
        throw OnceError.api(message)
    }
}
