def _surface_action_impl(ctx):
    output = ctx.actions.declare_output(ctx.attrs.out)
    command = cmd_args([
        "node",
        ctx.attrs._runner,
        "--output",
        output.as_output(),
        "--bytes",
        str(ctx.attrs.bytes),
        "--delay-ms",
        str(ctx.attrs.delay_ms),
        "--",
    ])
    command.add(ctx.attrs.srcs)
    ctx.actions.run(
        command,
        allow_cache_upload = True,
        category = "surface_action",
        identifier = ctx.label.name,
    )
    return [DefaultInfo(default_output = output)]

surface_action = rule(
    impl = _surface_action_impl,
    attrs = {
        "bytes": attrs.int(),
        "delay_ms": attrs.int(default = 40),
        "out": attrs.string(),
        "srcs": attrs.list(attrs.source()),
        "_runner": attrs.source(default = "//fixture:action.mjs"),
    },
)
