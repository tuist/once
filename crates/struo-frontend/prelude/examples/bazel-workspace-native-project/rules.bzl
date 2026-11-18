"""Minimal rules whose class names end in `_binary` and `_test` so the
example workspace exposes a `bazel_binary` and a `bazel_test` alongside
the `genrule` (which maps to `bazel_target`)."""

def _example_impl(ctx):
    out = ctx.actions.declare_file(ctx.label.name + ".ok")
    ctx.actions.write(out, "ok\n", is_executable = True)
    return [DefaultInfo(executable = out)]

example_binary = rule(
    implementation = _example_impl,
    executable = True,
)

example_test = rule(
    implementation = _example_impl,
    test = True,
)
