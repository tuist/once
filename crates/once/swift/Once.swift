import Foundation
import Once

public enum OnceError: Error, Equatable {
    case invalidUTF8
    case api(String)
}

public struct OnceCache: Sendable {
    public init() {
    }

    public var version: String {
        guard let raw = once_version() else {
            return ""
        }
        defer { once_string_free(raw) }
        return String(cString: raw)
    }

    public func digest(bytes: Data) async throws -> String {
        try await runAsync {
            return try bytes.withUnsafeBytes { buffer in
                let pointer = buffer.bindMemory(to: UInt8.self).baseAddress
                if let pointer {
                    return try decodeStringResponse(
                        once_digest_bytes(pointer, buffer.count)
                    )
                }
                return try decodeStringResponse(once_digest_bytes(nil, 0))
            }
        }
    }

    @discardableResult
    public func putBlob(_ bytes: Data) async throws -> String {
        let request = BlobPutRequest(bytes: Array(bytes))
        return try await decodeRequestAsync(request, with: once_cache_put_blob_json, as: String.self)
    }

    public func getBlob(_ digest: String) async throws -> Data {
        let request = DigestRequest(digest: digest)
        let response = try await decodeRequestAsync(request, with: once_cache_get_blob_json, as: BlobResponse.self)
        return Data(response.bytes)
    }

    public func hasBlob(_ digest: String) async throws -> Bool {
        let request = DigestRequest(digest: digest)
        return try await decodeRequestAsync(request, with: once_cache_has_blob_json, as: Bool.self)
    }

    @discardableResult
    public func putActionResult(_ result: OnceActionResult, for actionDigest: String) async throws -> Bool {
        let request = ActionResultPutRequest(
            actionDigest: actionDigest,
            result: result
        )
        return try await decodeRequestAsync(request, with: once_cache_put_action_result_json, as: Bool.self)
    }

    public func getActionResult(_ actionDigest: String) async throws -> OnceActionResult? {
        let request = ActionDigestRequest(actionDigest: actionDigest)
        return try await decodeRequestAsync(
            request,
            with: once_cache_get_action_result_json,
            as: OnceActionResult?.self
        )
    }

    @discardableResult
    public func forgetAction(_ actionDigest: String) async throws -> Bool {
        let request = ActionDigestRequest(actionDigest: actionDigest)
        return try await decodeRequestAsync(request, with: once_cache_forget_action_json, as: Bool.self)
    }

    public func stats() async throws -> OnceCacheStats {
        return try await decodeRequestAsync(EmptyRequest(), with: once_cache_stats_json, as: OnceCacheStats.self)
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

private struct EmptyRequest: Encodable, Sendable {
}

private struct BlobPutRequest: Encodable, Sendable {
    let bytes: [UInt8]
}

private struct BlobResponse: Decodable, Sendable {
    let bytes: [UInt8]
}

private struct DigestRequest: Encodable, Sendable {
    let digest: String
}

private struct ActionResultPutRequest: Encodable, Sendable {
    let actionDigest: String
    let result: OnceActionResult
}

private struct ActionDigestRequest: Encodable, Sendable {
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

private func runAsync<Value: Sendable>(
    _ operation: @escaping @Sendable () throws -> Value
) async throws -> Value {
    try Task.checkCancellation()
    let task = Task(operation: operation)
    return try await withTaskCancellationHandler {
        try await task.value
    } onCancel: {
        task.cancel()
    }
}

private func decodeRequestAsync<Request: Encodable & Sendable, Value: Decodable & Sendable>(
    _ request: Request,
    with function: CJSONFunction,
    as type: Value.Type
) async throws -> Value {
    try await runAsync {
        try decodeRequest(request, with: function, as: type)
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
