# frozen_string_literal: true

require "json"
require "once/native"

module Once
  class Error < StandardError; end

  ActionResult = Struct.new(:exit_code, :stdout, :stderr, :outputs, keyword_init: true) do
    def initialize(exit_code:, stdout: nil, stderr: nil, outputs: {})
      outputs ||= {}
      super(exit_code: exit_code, stdout: stdout, stderr: stderr, outputs: outputs.dup.freeze)
    end

    def to_h
      {
        exit_code: exit_code,
        stdout: stdout,
        stderr: stderr,
        outputs: outputs || {},
      }
    end
  end

  CacheStats = Struct.new(:blob_count, :blob_bytes, :action_count, :action_bytes, keyword_init: true)

  class Cache
    def initialize(local_cache_root: nil, workspace_root: nil)
      if local_cache_root && workspace_root
        raise ArgumentError, "local_cache_root and workspace_root cannot be used together"
      end

      @selection = {}
      @selection[:local_cache_root] = local_cache_root.to_s if local_cache_root
      @selection[:workspace_root] = workspace_root.to_s if workspace_root
      @selection.freeze
    end

    def version
      read_native_string(Native.once_version)
    end

    def digest(bytes)
      buffer = bytes.to_s.b
      pointer = memory_pointer(buffer)
      decode_response(Native.once_digest_bytes(pointer, buffer.bytesize))
    end

    def put_blob(bytes)
      buffer = bytes.to_s.b
      decode_request(:once_cache_put_blob_json, selection(bytes: buffer.bytes))
    end

    def put_file(path)
      decode_request(:once_cache_put_file_json, selection(path: path.to_s))
    end

    def get_blob(digest)
      response = decode_request(:once_cache_get_blob_json, selection(digest: digest))
      response.fetch("bytes").pack("C*")
    end

    def get_blob_to_file(digest, path)
      decode_request(
        :once_cache_get_blob_to_file_json,
        selection(digest: digest, path: path.to_s),
      )
    end

    def has_blob?(digest)
      decode_request(:once_cache_has_blob_json, selection(digest: digest))
    end

    def put_action_result(result, action_digest:)
      decode_request(
        :once_cache_put_action_result_json,
        selection(
          action_digest: action_digest,
          result: normalize_action_result(result),
        ),
      )
    end

    def get_action_result(action_digest)
      response = decode_request(
        :once_cache_get_action_result_json,
        selection(action_digest: action_digest),
      )
      response && action_result_from_native(response)
    end

    def forget_action(action_digest)
      decode_request(
        :once_cache_forget_action_json,
        selection(action_digest: action_digest),
      )
    end

    def stats
      response = decode_request(:once_cache_stats_json, @selection)
      CacheStats.new(
        blob_count: response.fetch("blob_count"),
        blob_bytes: response.fetch("blob_bytes"),
        action_count: response.fetch("action_count"),
        action_bytes: response.fetch("action_bytes"),
      )
    end

    private

    def selection(request)
      @selection.merge(request)
    end

    def decode_request(function, request)
      decode_response(Native.public_send(function, JSON.generate(request)))
    end

    def memory_pointer(buffer)
      pointer = FFI::MemoryPointer.new(:uint8, [buffer.bytesize, 1].max)
      pointer.put_bytes(0, buffer) unless buffer.empty?
      pointer
    end

    def decode_response(pointer)
      raise Error, "native Once function returned null" if pointer.null?

      response = JSON.parse(read_native_string(pointer, free: false))
      return response.fetch("value") if response.fetch("status") == "ok"

      raise Error, response.fetch("message")
    rescue JSON::ParserError, KeyError => e
      raise Error, e.message
    ensure
      Native.once_string_free(pointer) if pointer && !pointer.null?
    end

    def read_native_string(pointer, free: true)
      raise Error, "native Once function returned null" if pointer.null?

      pointer.read_string
    ensure
      Native.once_string_free(pointer) if free && pointer && !pointer.null?
    end

    def normalize_action_result(result)
      result = result.to_h if result.respond_to?(:to_h)
      {
        exit_code: result.fetch(:exit_code) { result.fetch("exit_code") },
        stdout: result[:stdout] || result["stdout"],
        stderr: result[:stderr] || result["stderr"],
        outputs: result[:outputs] || result["outputs"] || {},
      }
    end

    def action_result_from_native(result)
      ActionResult.new(
        exit_code: result.fetch("exit_code"),
        stdout: result["stdout"],
        stderr: result["stderr"],
        outputs: result.fetch("outputs", {}),
      )
    end
  end

  class ActionKey
    def initialize(namespace)
      raise ArgumentError, "namespace must be a String" unless namespace.is_a?(String)

      @namespace = namespace
      @inputs = []
    end

    def add_bytes(label, bytes)
      validate_label(label)
      @inputs << { kind: "bytes", label: label, bytes: bytes.to_s.b.bytes }
      self
    end

    def add_digest(label, digest)
      validate_label(label)
      @inputs << { kind: "digest", label: label, digest: digest }
      self
    end

    def digest
      pointer = Native.once_action_key_json(
        JSON.generate(namespace: @namespace, inputs: @inputs),
      )
      raise Error, "native Once function returned null" if pointer.null?

      response = JSON.parse(pointer.read_string)
      return response.fetch("value") if response.fetch("status") == "ok"

      raise Error, response.fetch("message")
    rescue JSON::ParserError, KeyError => e
      raise Error, e.message
    ensure
      Native.once_string_free(pointer) if pointer && !pointer.null?
    end

    private

    def validate_label(label)
      raise ArgumentError, "input label must be a String" unless label.is_a?(String)
    end
  end

  def self.digest(bytes)
    Cache.new.digest(bytes)
  end
end
