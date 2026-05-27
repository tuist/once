# Scripts

Scripts are Fabrik's bridge between existing repository automation and
the execution model underneath it.

The canonical script shape is `script`. You can keep the implementation
in a checked-in file and describe its contract through `FABRIK`
headers, or you can declare it directly in `fabrik.toml` when the
action is short enough to stay inline. Either way, Fabrik turns that
work into the same kind of action it uses elsewhere: explicit inputs,
declared outputs, content-addressed caching, and stable runtime
semantics.

That makes scripts the natural fit for packaging steps, generators,
integration setup, local tooling, and other operational work that
should be optimized without being rewritten as a language-specific rule.

## Choosing A Form

Use script files when the workflow already belongs in a script, when
the implementation is long enough that inline TOML would become
awkward, or when you want the cache contract to live next to the
operational logic itself. If the action is tiny and reads more cleanly
as manifest data, the inline form is usually a better fit.

## Next Pages

If your automation already lives in real script files, continue with
[Script Files](/guide/cacheable-scripts). If you want the manifest
surface and field semantics for inline declarations, go to
[Script Rules](/targets/scripts).
