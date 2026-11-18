# Microsandbox

[Microsandbox](https://github.com/microsandbox/microsandbox) runs a command in
an isolated micro-virtual-machine on the local computer. It is useful for
validating the same declared input and output contract used by the hosted E2B
and Daytona executors.

## Configure An Execution Environment

Give the infrastructure a name based on its role. The name is a policy label,
not the name of the test framework:

```toml
[infrastructures.remote_tests]
kind = "microsandbox"
image = "node:22.18.0-alpine"

[infrastructure.execution]
provider = "remote_tests"
```

The provider definition selects the Microsandbox executor adapter and the
image that supplies tools such as [Node.js](https://nodejs.org/). Pin the image
to an immutable digest for reproducible shared workflows.

Run one command through the configured provider:

```sh
struo exec --remote -- node --version
```

## Select The Provider From A Script

An annotated script can name the same infrastructure directly:

```sh
#!/bin/sh
# struo remote "remote_tests"
# struo input "../src"
# struo input "../tests"
# struo output "../coverage"
# struo cwd ".."

./node_modules/.bin/vitest run
```

Struo resolves `remote_tests` through the root manifest. The command uses the
Node.js executable from the configured image. Struo does not copy a
machine-specific interpreter path or import the host tool environment.

## Model Package Dependencies As An Action

The image should contain tools that are stable across many actions. Repository
dependencies belong in the action graph. For example, an install script can
use the [npm package manager](https://www.npmjs.com/) to produce `node_modules`:

```sh
#!/bin/sh
# struo input "../package.json"
# struo input "../package-lock.json"
# struo output "../node_modules"
# struo cwd ".."
# struo remote "remote_tests"

npm clean-install
```

The test script then declares the install script as a dependency:

```sh
# struo needs "./install.sh"
```

Struo runs the install action first. Its declared `node_modules` output becomes
a declared input of the test action and contributes to that action's cache
identity. A cache hit can restore the dependency output without running the
installer again.

This is preferable to copying the host's `node_modules` by default. Package
trees can contain platform-specific native files and symbolic links into a
package manager store. If an existing workflow must copy such a tree, declare
it as an `input`; Struo transfers it recursively and rejects symbolic links that
escape the declared input trees.

## Filesystem Boundary

The Microsandbox executor starts with a clean workspace. Before execution,
Struo creates the declared working directory and transfers only declared input
files, directories, and safe symbolic links. After a successful command, Struo
retrieves only declared outputs and installs them into the workspace through a
staging directory.

The local adapter streams these trees through the Microsandbox filesystem
service. Hosted E2B and Daytona adapters package the same declared trees into
archives for transfer. A future hosted executor can use a content-addressed
missing-file exchange without changing the script contract.

Struo stops the micro-virtual-machine and removes its persisted state after
success, failure, or timeout.

Microsandbox is currently available through Struo on Linux and Apple Silicon
macOS.
