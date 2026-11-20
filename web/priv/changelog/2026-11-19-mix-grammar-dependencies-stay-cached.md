---
title: Mix dependencies with parser grammars stay cached
date: 2026-11-19
---

Mix compiles `.yrl` and `.xrl` grammars into Erlang sources next to them,
inside the dependency's own directory. Struo used to count those generated
files as inputs, so dependencies such as `expo` and `earmark_parser`, and
everything that depends on them, compiled again on the next build after a
clean checkout. Struo now leaves the generated files out of the cache key,
and these dependencies are restored from the cache like the rest.
