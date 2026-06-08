# frozen_string_literal: true

require "tmpdir"

$LOAD_PATH.unshift(File.expand_path("lib", __dir__))
require "once"

Dir.mktmpdir("once-ruby-") do |dir|
  ENV["XDG_CACHE_HOME"] = dir

  cache = Once::Cache.new
  blob_digest = cache.put_blob("hello")

  raise "digest mismatch" unless Once.digest("hello") == blob_digest
  raise "blob missing" unless cache.has_blob?(blob_digest)
  raise "blob content mismatch" unless cache.get_blob(blob_digest) == "hello"

  action_digest = Once.digest("action")
  result = Once::ActionResult.new(
    exit_code: 0,
    stdout: blob_digest,
    stderr: nil,
    outputs: {},
  )

  raise "put action failed" unless cache.put_action_result(result, action_digest: action_digest)
  raise "action mismatch" unless cache.get_action_result(action_digest) == result
  raise "forget failed" unless cache.forget_action(action_digest)
  raise "action still present" unless cache.get_action_result(action_digest).nil?
  raise "stats failed" unless cache.stats.blob_count.is_a?(Integer)
end
