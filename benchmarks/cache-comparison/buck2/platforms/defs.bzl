def _platforms(ctx):
    configuration = ConfigurationInfo(
        constraints = {},
        values = {},
    )
    platform = ExecutionPlatformInfo(
        label = ctx.label.raw_target(),
        configuration = configuration,
        executor_config = CommandExecutorConfig(
            allow_cache_uploads = True,
            local_enabled = True,
            max_cache_upload_mebibytes = 64,
            remote_cache_enabled = True,
            remote_enabled = False,
            use_limited_hybrid = False,
        ),
    )
    return [
        DefaultInfo(),
        ExecutionPlatformRegistrationInfo(platforms = [platform]),
    ]

platforms = rule(attrs = {}, impl = _platforms)
