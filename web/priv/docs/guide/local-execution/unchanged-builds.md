# Unchanged Builds

Most builds have nothing to do. The interesting question is how quickly Once can
establish that, because an answer that takes seconds is one you stop waiting for.

Once establishes it in one of two ways, and the difference is whether something
is watching the working copy.

## Being Told What Changed

Once starts a small filesystem change tracker for a working copy the first time
it builds there. The tracker watches sources and build outputs and keeps a
counter for each. A build records the counters it saw alongside its result; the
next build compares two numbers, and when neither has moved there is nothing to
re-derive, re-analyse, or re-check.

This is the fast path, and it is fast because it asks one question instead of
thousands. Building an unchanged target in a large workspace takes a few tens of
milliseconds.

The tracker exits on its own when the working copy goes quiet, and starting it
again costs one slower build.

## Asking The Filesystem

Without a tracker, Once has to ask. It compares the description of every source
file, dependency tree, and output the previous build read or produced: size,
mode, modification time, and inode change time. Nothing is read, but everything
is looked at, and the cost grows with the size of the workspace rather than with
the size of the change.

Once takes this path when the tracker is unavailable, and it produces the same
answer either way. It is only slower.

To skip the tracker deliberately, set `ONCE_CHANGE_TRACKER=0`:

```sh
ONCE_CHANGE_TRACKER=0 once build app
```

A one-shot container is the case where this is worth doing: there is no series
of builds to amortise the tracker's startup over, so the tracker is a cost with
no payer.

## What A Description Settles

Comparing descriptions rather than contents rests on the inode change time. A
process can set a file's modification time to whatever it likes, which is how
copying with preserved timestamps, `rsync --times`, and unpacking an archive all
present new content under an old timestamp. No process can set the change time,
because the operating system stamps it on every write to the file. A description
that includes it therefore moves whenever the file does.

To compare contents instead, set `ONCE_TREE_DIGEST_CACHE=0`. Every dependency
tree is then read in full on every build, which is slower and answers only to
the bytes.

## Reusing A Target

A build that recompiles one file still has to decide what to do about every
target the changed one depends on. Once records what building each target
produced, together with the files it read and the patterns its target kind
expanded, and reuses that record when the watcher reports that none of them
moved and no dependency of it was rebuilt.

A dependency that rebuilt gives everything above it a different name, so the
chain re-forms from the change upwards. A target whose actions declined to be
cached is never recorded, because skipping its work is the one thing it asked
not to happen.

Set `ONCE_TARGET_OUTCOMES=0` to visit every target, which is useful when
confirming that a result came from building rather than from a record of it.

## Reusing Graph Derivation

A project with no Once targets of its own has its graph derived from its package
manifests, which means running the package manager's resolver. Once records the
derived graph along with the manifests it read and the patterns that selected
them, and reuses it when the watcher reports that none of them moved.

Set `ONCE_RESOLUTION_CACHE=0` to derive the graph every time.

## Reusing Analysis

A target's Once definition is turned into commands by its target kind, written in
Starlark. That work is remembered too: alongside the commands, Once records every
answer the target kind got from outside itself, such as an environment variable,
a tool resolved on `PATH`, the output of a version probe, or the files a glob
matched. Next time, if the target definition and its dependencies' results are
the same and every recorded answer is still the answer the host gives, the
commands are reused rather than derived again.

An answer Once cannot describe is never recorded, and a target whose analysis
produced one is analysed afresh every time. That is deliberate: a record that
cannot be checked is worse than no record.

Set `ONCE_ANALYSIS_MEMO=0` to derive commands every time, which is useful when
confirming that a result came from a target kind rather than from a record of it.
