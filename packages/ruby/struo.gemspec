Gem::Specification.new do |spec|
  spec.name = "buildstruo"
  spec.version = ENV.fetch("STRUO_VERSION", "0.0.0")
  spec.summary = "Ruby software development kit for Struo"
  spec.description = "Embeds Struo primitives in Ruby through the native Struo library."
  spec.license = "MIT"
  spec.homepage = "https://github.com/tuist/struo"
  spec.metadata = {
    "source_code_uri" => "https://github.com/tuist/struo",
  }
  spec.authors = ["Tuist GmbH"]
  spec.required_ruby_version = ">= 3.1"
  spec.files = Dir[
    "README.md",
    "lib/**/*.rb",
    "prebuilds/**/*",
  ].select { |path| File.file?(path) }
  spec.require_paths = ["lib"]
  spec.add_dependency "ffi", "~> 1.16"
end
