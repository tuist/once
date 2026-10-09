pub(crate) fn to_wire(
    value: once_core::ActionPresentation,
) -> Option<crate::proto::ActionPresentation> {
    let value = value.normalized()?;
    Some(crate::proto::ActionPresentation {
        package: value.package.map(|package| crate::proto::ActionPackage {
            ecosystem: package.ecosystem,
            name: package.name,
            version: package.version,
            revision: package.revision,
            digest: package.digest,
            origin: package.origin,
        }),
        platforms: value
            .platforms
            .into_iter()
            .map(|platform| crate::proto::ActionPlatform {
                scheme: platform.scheme,
                id: platform.id,
                label: platform.label,
                usage: platform.usage,
            })
            .collect(),
        context: value
            .context
            .into_iter()
            .map(|context| crate::proto::ActionContext {
                key: context.key,
                value: context.value,
                label: context.label,
            })
            .collect(),
    })
}
