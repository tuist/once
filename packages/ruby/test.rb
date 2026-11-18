# frozen_string_literal: true

require "tmpdir"
require "open3"

$LOAD_PATH.unshift(File.expand_path("lib", __dir__))
require "buildstruo"

def assert_raises_struo_error
  yield
rescue Struo::Error
  return
else
  raise "expected Struo::Error"
end

def assert_command_fails(command, env = {})
  _stdout, stderr, status = Open3.capture3(env, *command)
  raise "expected command to fail" if status.success?
  raise "expected missing library failure" unless stderr.match?(/missing|cannot open|no such file/i)
end

Dir.mktmpdir("struo-ruby-") do |dir|
  ENV["XDG_CACHE_HOME"] = dir

  cache = Struo::Cache.new
  blob_digest = cache.put_blob("hello")

  raise "digest mismatch" unless Struo.digest("hello") == blob_digest
  raise "empty digest mismatch" unless Struo.digest("") == cache.put_blob("")
  raise "utf8 digest mismatch" unless Struo.digest("é") == Struo.digest("é".encode(Encoding::UTF_8))
  raise "blob missing" unless cache.has_blob?(blob_digest)
  raise "blob content mismatch" unless cache.get_blob(blob_digest) == "hello"
  raise "empty blob mismatch" unless cache.get_blob(Struo.digest("")) == ""

  explicit_cache = Struo::Cache.new(local_cache_root: File.join(dir, "explicit-cache"))
  input_path = File.join(dir, "input.bin")
  output_path = File.join(dir, "nested", "output.bin")
  File.binwrite(input_path, "file payload")
  file_digest = explicit_cache.put_file(input_path)
  raise "file byte count mismatch" unless explicit_cache.get_blob_to_file(file_digest, output_path) == 12
  raise "file content mismatch" unless File.binread(output_path) == "file payload"
  raise "explicit cache leaked into default cache" if cache.has_blob?(file_digest)
  begin
    Struo::Cache.new(local_cache_root: dir, workspace_root: dir)
    raise "expected conflicting cache roots to fail"
  rescue ArgumentError
  end

  action_key = Struo::ActionKey.new("compile")
                              .add_bytes("tool", "swiftc")
                              .add_digest("source", Struo.digest("source"))
  raise "action key is unstable" unless action_key.digest == action_key.digest
  other_key = Struo::ActionKey.new("link")
                            .add_bytes("tool", "swiftc")
                            .add_digest("source", Struo.digest("source"))
  raise "action key namespace was ignored" if action_key.digest == other_key.digest
  assert_raises_struo_error { cache.get_blob("not-a-digest") }
  assert_raises_struo_error { cache.has_blob?("not-a-digest") }

  action_digest = Struo.digest("action")
  result = Struo::ActionResult.new(
    exit_code: 0,
    stdout: blob_digest,
    stderr: nil,
    outputs: {},
  )
  raise "outputs should be frozen" unless result.outputs.frozen?

  raise "put action failed" unless cache.put_action_result(result, action_digest: action_digest)
  raise "action mismatch" unless cache.get_action_result(action_digest) == result
  raise "forget failed" unless cache.forget_action(action_digest)
  raise "action still present" unless cache.get_action_result(action_digest).nil?
  assert_raises_struo_error { cache.put_action_result(result, action_digest: "not-a-digest") }
  assert_raises_struo_error { cache.get_action_result("not-a-digest") }
  assert_raises_struo_error { cache.forget_action("not-a-digest") }
  raise "stats failed" unless cache.stats.blob_count.is_a?(Integer)

  %w[one two three].map { |value| Thread.new { cache.put_blob(value) } }.each(&:value)

  assert_command_fails(
    [
      RbConfig.ruby,
      "-I#{File.expand_path("lib", __dir__)}",
      "-e",
      "require 'buildstruo'",
    ],
    "STRUO_LIBRARY_PATH" => "/missing/libstruo.dylib",
  )
end
