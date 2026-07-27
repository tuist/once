def _surface_action_impl(ctx):
    output = declare_output(ctx["attr"]["out"])
    inputs = glob(ctx["srcs"])
    inputs.append("fixture/action.mjs")
    for dependency in ctx["deps"]:
        artifact = dependency.get("benchmark_artifact")
        if artifact:
            inputs.append(artifact)
    inputs = sorted(inputs)
    argv = [
        "node",
        execution_path("fixture/action.mjs"),
        "--output",
        execution_path(output),
        "--bytes",
        str(ctx["attr"]["bytes"]),
        "--delay-ms",
        str(ctx["attr"]["delay_ms"]),
        "--",
    ] + inputs
    run_action(
        argv = argv,
        inputs = inputs,
        outputs = [output],
        sandbox = "inputs",
        toolchain_identity = host_command(["node", "--version"]),
        identifier = ctx["label"]["id"] + ":surface",
    )
    return {
        "label_id": ctx["label"]["id"],
        "benchmark_artifact": output,
    }

surface_action = target_kind(
    docs = "Runs one deterministic action in the cache comparison graph.",
    attrs = [
        attr("bytes", "int", required = True, configurable = False),
        attr("delay_ms", "int", default = "40", configurable = False),
        attr("out", "string", required = True, configurable = False),
    ],
    deps = [
        dep(
            "deps",
            ["benchmark_artifact"],
            "Surface artifacts consumed by this action.",
        ),
    ],
    providers = ["benchmark_artifact"],
    capabilities = [capability("build", ["default"])],
    tools = [tool("node", executables = ["node"])],
    impl = _surface_action_impl,
)
