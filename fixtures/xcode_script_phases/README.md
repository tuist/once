# Native script phases

One tiny Swift application exercises the execution environment and ordering of
Xcode script phases. Preparation reads a native input file list, a cached
product script writes a packaged resource, and an always-run script edits the
property list and creates an undeclared resource before signing. A generator
between compilation and packaging consumes the linked executable and produces
a file and a resource bundle. A subsequent cached script consumes the bundle
modified by the always-run script. An installation-only phase must not run
during an ordinary build.

The acceptance test checks per-action cache outcomes, repeated always-run
execution, signature validity, output restoration, paths containing spaces,
and a configuration-specific output directory. It does not launch a simulator
or resolve dependencies.

After building the release executable:

```sh
mise exec -- shellspec spec/xcode_script_phases_spec.sh
```
