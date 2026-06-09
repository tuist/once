Gem::Specification.new do |spec|
  spec.name = "buildonce"
  spec.version = ENV.fetch("ONCE_VERSION", "0.0.0")
  spec.summary = "Ruby SDK for Once"
  spec.description = "Embeds Once primitives in Ruby through the native Once library."
  spec.license = "MIT"
  spec.homepage = "https://github.com/tuist/once"
  spec.metadata = {
    "source_code_uri" => "https://github.com/tuist/once",
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
