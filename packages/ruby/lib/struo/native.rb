# frozen_string_literal: true

require "ffi"
require "rbconfig"

module Struo
  module Native
    extend FFI::Library

    def self.library_path
      return ENV.fetch("STRUO_LIBRARY_PATH") if ENV["STRUO_LIBRARY_PATH"]

      candidate = File.expand_path(
        File.join("..", "..", "prebuilds", platform_key, library_name),
        __dir__,
      )
      return candidate if File.file?(candidate)

      raise LoadError,
            "missing native Struo library for #{platform_key}; set STRUO_LIBRARY_PATH or install a gem that includes this platform"
    end

    def self.platform_key
      host = RbConfig::CONFIG.fetch("host_os")
      arch = RbConfig::CONFIG.fetch("host_cpu")
      os = case host
           when /darwin/
             "darwin"
           when /mswin|mingw|cygwin/
             "win32"
           when /linux/
             "linux"
           else
             host
           end
      cpu = case arch
            when /arm64|aarch64/
              "arm64"
            when /x86_64|amd64/
              "x64"
            else
              arch
            end
      "#{os}-#{cpu}"
    end

    def self.library_name
      case platform_key
      when /^darwin-/
        "libstruo.dylib"
      when /^win32-/
        "struo.dll"
      else
        "libstruo.so"
      end
    end

    ffi_lib library_path

    attach_function :struo_version, [], :pointer
    attach_function :struo_string_free, [:pointer], :void
    attach_function :struo_digest_bytes, %i[pointer size_t], :pointer
    attach_function :struo_action_key_json, [:string], :pointer
    attach_function :struo_cache_put_blob_json, [:string], :pointer
    attach_function :struo_cache_put_file_json, [:string], :pointer
    attach_function :struo_cache_get_blob_json, [:string], :pointer
    attach_function :struo_cache_get_blob_to_file_json, [:string], :pointer
    attach_function :struo_cache_has_blob_json, [:string], :pointer
    attach_function :struo_cache_put_action_result_json, [:string], :pointer
    attach_function :struo_cache_get_action_result_json, [:string], :pointer
    attach_function :struo_cache_forget_action_json, [:string], :pointer
    attach_function :struo_cache_stats_json, [:string], :pointer
  end
end
