def surface_action(name, srcs, bytes, delay_ms = 40):
    native.genrule(
        name = name,
        srcs = srcs,
        outs = [name + ".bin"],
        tools = ["//fixture:action"],
        cmd = "node $(location //fixture:action) --output $@ --bytes {} --delay-ms {} -- $(SRCS)".format(bytes, delay_ms),
    )
