# Request for Comments 0011: Dockerfile instruction actions

## Decision

Existing Dockerfiles are an input language for Once actions. Users should
not have to rewrite recipes as image-layer target declarations to gain
instruction-level caching and scheduling.

The target module parses instruction boundaries, argument scope, source
locations, and stage references. It declares one snapshot action per executed
instruction, with edges expressed through consumed snapshot directories.
BuildKit executes the individual instruction against imported image layouts,
passed as `oci-layout://` build contexts with no digest. The final archive is
a separate export action. Rust knows only ordinary commands, directories, file
materialization, scheduling, and cache contracts. No interpreter or helper
script participates; each step is one `docker buildx build` argv.

Stage arguments and environment are resolved during analysis. A stage inherits
its parent stage's arguments, a supplied build argument wins at every
declaration, `ARG name=default` replaces the current value, and a bare
`ARG name` keeps it or takes the global value. Each step's definition
re-declares the resolved values, so no state travels between steps. A default
that depends on variables only the base image knows cannot be resolved and
selects whole-file execution. `ARG` instructions themselves declare no action.

## Execution contract

Actions use private copies of declared inputs. Literal copy sources narrow
the context input set; variable and wildcard sources and context bind mounts
remain conservative. Stage arguments are retained with snapshots because
they are not part of image runtime configuration. Unreachable stages do not
execute. Mutable worker cache mounts are optimization state and are not
snapshot outputs.
Intermediate image layers preserve filesystem timestamps, including generated
source files, because normalizing intermediate files can change compiler
regeneration decisions. The cost is that BuildKit skips timestamp rewriting for
layers inherited from the imported base, so the final archive keeps the
timestamps from each instruction's run. Image creation time is still fixed by
`source_date_epoch`. Byte-identical layers across independent rebuilds are an
explicit opt-in, `reproducible_layers`, which rewrites each instruction's own
layer as it is exported and therefore gives later instructions normalized
timestamps.

Triggers stored in a base image by `ONBUILD` run in the `FROM` step and need
the whole context, which instruction translation does not give that step. The
target kind therefore asks the local engine, then the registry, whether a base
declares triggers and selects whole-file execution when it does. When neither
can be reached the answer is unknown and the build proceeds, so such a base
should select whole-file execution explicitly. Symbolic links in copy sources
declare their destinations, and empty directories are inputs, so copied trees
keep them.

Staging copies files with all of their permission bits, which participate in
input digests, and gives the directories above a staged input the modes they
have in the workspace, set after staging so a restrictive parent cannot block
its siblings. A directory's own mode is not part of the digest of the files
inside it, so changing only that mode does not invalidate a cached result.

File permission bits participate in input digests. Without that, making a
copied script executable would keep serving the image built from the
non-executable file.

Definitions are declared before command actions. Commands follow dependency
depth order so independent stages stay adjacent and can share bounded
scheduler batches. Their dependencies remain ordinary snapshot inputs.

The explicit cacheability decision applies to each instruction. Pulling
mutable base images and reusing Once action results are incompatible, and a
cacheable declaration must name its platform. BuildKit workers require their
own memory budgets. The Once client stays within generic action admission.

Remote BuildKit builders execute the same individual instruction contracts
and return their declared snapshots. This does not introduce a Docker-specific
remote provider or new graph remote-execution policy in Rust.

## Input guidance

The existing lint capability reports advisory findings about mutable base
references, repository updates, network pipelines, and authentication mounts.
Findings identify the target, attribute, instruction line, and suggested
repair. They are evidence for agent review, not a proof of reproducibility
and not an implicit cache-policy override.

## Compatibility

Whole-file execution remains available through `execution_mode = "buildkit"`.
Extended frontend instructions, heredocs, deferred `ONBUILD` instructions,
and extended argument expressions use this path. The default `auto` mode
selects it when the parser records unsupported syntax or a whole-file-only
option is set, and reports the reason as a lint note. Explicit
`instructions` mode fails at build time with the location and this exact
setting. Lint and metadata never fail on unsupported syntax. The
compatibility path retains portable BuildKit cache imports and exports.

Cacheable execution requires every base image of a built stage to be pinned by
digest, because the cache key does not include what a tag resolves to.

Image snapshots currently duplicate the complete layout at each instruction.
Measured on a four-megabyte image with eight snapshots, the output directory
held 32 megabytes, one full layout per step. The cost grows with the image and
the instruction count, so a large base image with many instructions is better
served by whole-file execution, which keeps one layout.
Content-addressed artifact storage can deduplicate equal blobs, but local
materialization and BuildKit import/export still cost disk space and time.
Validation must include a multi-stage application recipe and selective
invalidation, not only a successful final archive export.
