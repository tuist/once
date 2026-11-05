# Cargo semantic parity experiment (E7)

This is an internal, bounded differential harness, not a new resolver or a
semantic equivalence proof. It exercises the existing Cargo target kind without
changing its architecture. Python 3.11+, Cargo and Rust 1.96.0, a C linker, procps
`pkill`, and a built Once executable are required. The host and target must both be
`x86_64-unknown-linux-gnu`.

From this directory, with the pinned toolchain active:

```sh
python3 parity.py --once /absolute/path/to/once --output /tmp/e7-evidence --seeds
```

The recorder's fast regression check uses real rustc and needs no Once build:

```sh
python3 -B -m unittest discover -s . -p 'test_*.py'
```

The output directory must not exist. Exit 0 means the modeled observations agree,
both controls succeeded, the two Once graph fingerprints agree, and every
requested mutation was detected in its required observation category. Exit 1
means a discrepancy or failed acceptance gate. A successful mutation campaign
does not make a discrepant baseline pass. Unexpected harness failures raise an
exception instead of reporting parity.

## Independent observations

Both arms receive separate disposable copies of each fixture. Native Cargo runs
`build` and `test --tests`, with `--locked --offline --no-default-features
--features extra --target x86_64-unknown-linux-gnu --profile dev -j 1`.
The Once seed requests the same target and features through `once.toml`. The
Cargo manifest pins dev optimization to 0, debuginfo to 2 and incremental to false.
Once receives a 2560 MiB local memory budget and one test worker. Native tests
and direct libtest reruns use one thread; Once's runner keeps its default test
environment. No remote provider or production service is involved.
The complex workspace explicitly limits `default-members` to its root package;
one dependency has its own test so extra test selection is observable.

A disposable compiler proxy forwards the original argv and selected environment
unchanged to the real compiler. Its sysroot probe points Once to that proxy;
the real sysroot libraries remain available. This observes actual compiler
invocations from both engines, not a second interpretation of Once's resolver
metadata. Each successful invocation is independently replayed for dep-info and
queried for effective cfg. These observation-only replays do not execute tests
or build scripts, and their outputs are private temporary files.

The comparator checks, symmetrically:

- Build-phase unit membership, captured before testing, so later test compilation
  cannot hide an incomplete successful build.
- Compiler-selected source and included-file membership, relative to the owning
  package, rather than the resolver's declared source globs.
- Actual direct `--extern` aliases for each package/root/test compilation unit.
  Build scripts identify build dependencies; libtest units identify test/dev
  consumers. The complex fixture uses different normal, build and dev packages
  so a role mix-up cannot pass accidentally.
- Effective cfg, edition, target, crate type, and selected codegen settings.
- Consumed generated inputs, including their names, text and SHA-256 digests.
  Only the fixture's deterministic generated text is byte-compared. Executables
  and archives are not assumed byte-identical.
- Test binaries and their cases/statuses, including an ignored case. The
  executors' actual results are checked separately from directly rerunning each
  observed libtest binary: Cargo's test transcript versus Once's normalized
  result queries. This distinguishes compilation scope from executed scope.

Raw argv, selected environment, compiler outcomes, executor logs and normalized
results remain in the evidence directory. Generated runner compilations without
a native workload counterpart are counted as unmatched support observations,
not silently declared equivalent. Repeated graph fingerprints are a separate
repeatability observation; they cannot establish native parity.

## Interpreting the baseline

This is an observational gate, not an assertion that all differences are bugs.
Once deliberately runs all first-party workspace tests by default, whereas Cargo
honors `default-members`. The complex fixture exposes that policy difference.
Once also does not currently promise to resolve the Cargo dev profile into
identical compiler flags. The harness reports debuginfo and embedded-bitcode
differences without overriding either engine's flags to hide them. In particular,
it does not change the build-script dev-dependency fallback policy.

Consequently a working harness and five detected mutations can accompany exit 1
from the baseline parity gate. Read `controls_succeeded`, `seed_detection` and
the per-field `differences` separately. A new unsupported scope must not be
described as verified equivalence.

## Seeded errors

Mutations affect only a disposable Once-arm input/configuration copy, not the
production resolver. Each must add a discrepancy beyond the unmodified baseline:

| Seed | Mutation | Required detection |
| --- | --- | --- |
| feature | Remove `extra` from Once's selection | Effective cfg |
| source | Inline the same constant instead of compiling its module | Source membership, while tests still pass |
| build-dependency | Replace the generator dependency with equivalent inline parsing | Build-script externs, while tests still pass |
| generated-file | Generate a different filename while the consumer still needs the original | Unavailable generated-input observation, plus compiler's missing-file error |
| test | Disable integration-test autodiscovery | Test scope, while remaining tests still pass |

## Limits

This harness intentionally accepts only two small Linux fixtures and one pinned
selection. It does not cover doctests, benchmarks, examples, procedural macros,
registry/git sources, native linking semantics, cross compilation, release
profiles, arbitrary Cargo configuration, remote execution, sandbox confinement,
cache replay or incremental invalidation. Dep-info parsing assumes the controlled
fixture paths contain no whitespace or Make escapes. Codegen comparison is a
named projection, not full argv equality; raw identity/output/search-path and
diagnostic flags are retained but not treated as portable semantic equality.
Failed compilations report source/generated observations as `null` (unknown),
not as an established empty input set. Compiler stderr preserves the reason.

Unit matching uses package, source root and test mode only. Host/target twins or
multiple profile variants sharing that identity are unsupported, even when their
observed settings happen to agree. Conflicting repeated observations abort rather
than overwrite one another. Keep the fixtures' normal, build and dev dependencies
distinct; do not extend this key into a general Cargo unit-graph implementation.
The pinned compiler's known defaults are used only for the named codegen
projection; this is not a resolved native profile contract.

All trials run serially. Each command has a 180-second timeout; compiler
observation probes have 30-second timeouts. Timed-out commands have their dedicated
session killed before workspace cleanup, including Once's separate action process
groups. Processes escaping the session are unsupported. Logs go to files, with a 4 MiB
post-run fixture-size check; generated text is limited to 1 MiB. This is not a
hard process RSS or hostile-output confinement mechanism. Binary hashing streams
through bounded buffers. Executables, caches, compiler proxies and temporary
workspaces are deleted after a successful campaign; useful observations remain.
