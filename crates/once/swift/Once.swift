import Foundation
import Once

public enum OnceError: Error, Equatable {
    case invalidUTF8
    case api(String)
}

public struct OnceCache: Sendable {
    private let localCacheRoot: String?
    private let workspaceRoot: String?

    /// Creates a cache client backed by the default base-directory cache location.
    ///
    /// The local cache root is `$XDG_CACHE_HOME/once/cas` when
    /// `XDG_CACHE_HOME` is set, and `$HOME/.cache/once/cas` otherwise.
    public init() {
        self.localCacheRoot = nil
        self.workspaceRoot = nil
    }

    /// Creates a cache client backed by a caller-owned local root.
    public init(localCacheRoot: URL) {
        self.localCacheRoot = localCacheRoot.path
        self.workspaceRoot = nil
    }

    /// Creates a cache client using the effective provider for a workspace.
    public init(workspaceRoot: URL) {
        self.localCacheRoot = nil
        self.workspaceRoot = workspaceRoot.path
    }

    /// Returns the linked Once version.
    public var version: String {
        guard let raw = once_version() else {
            return ""
        }
        defer { once_string_free(raw) }
        return String(cString: raw)
    }

    /// Returns the content digest for bytes without writing them to the cache.
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

    /// Stores bytes and returns their content digest.
    @discardableResult
    public func putBlob(_ bytes: Data) async throws -> String {
        let request = BlobPutRequest(
            localCacheRoot: localCacheRoot,
            workspaceRoot: workspaceRoot,
            bytes: Array(bytes)
        )
        return try await decodeRequestAsync(request, with: once_cache_put_blob_json, as: String.self)
    }

    /// Stores a file without loading its complete contents into Swift memory.
    @discardableResult
    public func putBlob(contentsOf url: URL) async throws -> String {
        let request = FilePutRequest(
            localCacheRoot: localCacheRoot,
            workspaceRoot: workspaceRoot,
            path: url.path
        )
        return try await decodeRequestAsync(request, with: once_cache_put_file_json, as: String.self)
    }

    /// Reads bytes for a digest.
    public func getBlob(_ digest: String) async throws -> Data {
        let request = DigestRequest(
            localCacheRoot: localCacheRoot,
            workspaceRoot: workspaceRoot,
            digest: digest
        )
        let response = try await decodeRequestAsync(request, with: once_cache_get_blob_json, as: BlobResponse.self)
        return Data(response.bytes)
    }

    /// Materializes a blob at a file location and returns the number of bytes written.
    @discardableResult
    public func getBlob(_ digest: String, writeTo url: URL) async throws -> UInt64 {
        let request = BlobFileRequest(
            localCacheRoot: localCacheRoot,
            workspaceRoot: workspaceRoot,
            digest: digest,
            path: url.path
        )
        return try await decodeRequestAsync(
            request,
            with: once_cache_get_blob_to_file_json,
            as: UInt64.self
        )
    }

    /// Returns whether a blob exists.
    public func hasBlob(_ digest: String) async throws -> Bool {
        let request = DigestRequest(
            localCacheRoot: localCacheRoot,
            workspaceRoot: workspaceRoot,
            digest: digest
        )
        return try await decodeRequestAsync(request, with: once_cache_has_blob_json, as: Bool.self)
    }

    /// Stores a cached result for an action digest.
    @discardableResult
    public func putActionResult(_ result: OnceActionResult, for actionDigest: String) async throws -> Bool {
        let request = ActionResultPutRequest(
            localCacheRoot: localCacheRoot,
            workspaceRoot: workspaceRoot,
            actionDigest: actionDigest,
            result: result
        )
        return try await decodeRequestAsync(request, with: once_cache_put_action_result_json, as: Bool.self)
    }

    /// Returns a cached result when one exists.
    public func getActionResult(_ actionDigest: String) async throws -> OnceActionResult? {
        let request = ActionDigestRequest(
            localCacheRoot: localCacheRoot,
            workspaceRoot: workspaceRoot,
            actionDigest: actionDigest
        )
        return try await decodeRequestAsync(
            request,
            with: once_cache_get_action_result_json,
            as: OnceActionResult?.self
        )
    }

    /// Removes one cached action result.
    @discardableResult
    public func forgetAction(_ actionDigest: String) async throws -> Bool {
        let request = ActionDigestRequest(
            localCacheRoot: localCacheRoot,
            workspaceRoot: workspaceRoot,
            actionDigest: actionDigest
        )
        return try await decodeRequestAsync(request, with: once_cache_forget_action_json, as: Bool.self)
    }

    /// Returns local cache statistics.
    public func stats() async throws -> OnceCacheStats {
        let request = CacheRequest(
            localCacheRoot: localCacheRoot,
            workspaceRoot: workspaceRoot
        )
        return try await decodeRequestAsync(request, with: once_cache_stats_json, as: OnceCacheStats.self)
    }
}

public struct OnceActionKey: Sendable {
    private let namespace: String
    private var inputs: [ActionKeyInputRequest]

    public init(namespace: String) {
        self.namespace = namespace
        self.inputs = []
    }

    public mutating func add(bytes: Data, label: String) {
        inputs.append(
            ActionKeyInputRequest(
                kind: "bytes",
                label: label,
                bytes: Array(bytes),
                digest: nil
            )
        )
    }

    public mutating func add(digest: String, label: String) {
        inputs.append(
            ActionKeyInputRequest(
                kind: "digest",
                label: label,
                bytes: nil,
                digest: digest
            )
        )
    }

    public func digest() async throws -> String {
        let request = ActionKeyRequest(namespace: namespace, inputs: inputs)
        return try await decodeRequestAsync(request, with: once_action_key_json, as: String.self)
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

private struct CacheRequest: Encodable, Sendable {
    let localCacheRoot: String?
    let workspaceRoot: String?
}

private struct BlobPutRequest: Encodable, Sendable {
    let localCacheRoot: String?
    let workspaceRoot: String?
    let bytes: [UInt8]
}

private struct FilePutRequest: Encodable, Sendable {
    let localCacheRoot: String?
    let workspaceRoot: String?
    let path: String
}

private struct BlobResponse: Decodable, Sendable {
    let bytes: [UInt8]
}

private struct DigestRequest: Encodable, Sendable {
    let localCacheRoot: String?
    let workspaceRoot: String?
    let digest: String
}

private struct BlobFileRequest: Encodable, Sendable {
    let localCacheRoot: String?
    let workspaceRoot: String?
    let digest: String
    let path: String
}

private struct ActionResultPutRequest: Encodable, Sendable {
    let localCacheRoot: String?
    let workspaceRoot: String?
    let actionDigest: String
    let result: OnceActionResult
}

private struct ActionDigestRequest: Encodable, Sendable {
    let localCacheRoot: String?
    let workspaceRoot: String?
    let actionDigest: String
}

private struct ActionKeyRequest: Encodable, Sendable {
    let namespace: String
    let inputs: [ActionKeyInputRequest]
}

private struct ActionKeyInputRequest: Encodable, Sendable {
    let kind: String
    let label: String
    let bytes: [UInt8]?
    let digest: String?
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
