use crate::{Action, Error, NetworkPolicy, RemoteExecution, Result};

/// The capacity an executor consumes, not its execution-platform identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResourceDomain {
    Host,
    /// Outstanding client requests. The service owns physical worker admission.
    Remote,
}

/// A resolved execution adapter. Aliases and tenancy remain in `RemoteExecution`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Executor {
    Local,
    Microsandbox,
    E2b,
    Daytona,
    Docker,
}

impl Executor {
    pub fn resolve(action: &Action) -> Result<Self> {
        let Action::RunCommand {
            remote: Some(binding),
            network,
            stdout_path,
            stderr_path,
            output_symlink_mode,
            ..
        } = action
        else {
            return Ok(Self::Local);
        };
        let executor = Self::from_binding(binding)?;
        if executor != Self::Local {
            if *network == NetworkPolicy::Deny {
                return Err(executor.unsupported("network denial"));
            }
            if stdout_path.is_some() || stderr_path.is_some() {
                return Err(executor.unsupported("stdout/stderr path redirection"));
            }
            if !output_symlink_mode.is_default() {
                return Err(executor.unsupported("non-default output symlink policy"));
            }
        }
        Ok(executor)
    }

    pub(crate) fn unsupported(self, requirement: &'static str) -> Error {
        Error::UnsupportedExecutorRequirement {
            executor: self.name().to_string(),
            requirement,
        }
    }

    pub fn from_binding(binding: &RemoteExecution) -> Result<Self> {
        let executor = match binding.executor.as_deref().unwrap_or(&binding.provider) {
            "local" if binding.environment.is_none() => Ok(Self::Local),
            "local" => Err(Error::RemoteProviderConfig {
                provider: binding.provider.clone(),
                message: "local execution cannot supply an image or template; select an executor that provides the requested environment".to_string(),
            }),
            "microsandbox" => Ok(Self::Microsandbox),
            "e2b" => Ok(Self::E2b),
            "daytona" => Ok(Self::Daytona),
            "docker" => Ok(Self::Docker),
            adapter => Err(Error::UnsupportedRemoteProvider {
                provider: adapter.to_string(),
            }),
        }?;
        if binding.account.is_some() || binding.project.is_some() {
            return Err(executor.unsupported("explicit account/project execution scoping"));
        }
        if executor == Self::Docker
            && !binding.environment.as_deref().is_some_and(|image| {
                image.strip_prefix("sha256:").is_some_and(|digest| {
                    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            })
        {
            return Err(Error::RemoteProviderConfig {
                provider: binding.provider.clone(),
                message: "Docker requires an explicit immutable sha256:<64 hex digits> image ID in environment; warm the image first, then resolve it with: docker image inspect --format '{{.Id}}' IMAGE".into(),
            });
        }
        Self::validate_default_environment(binding, executor, |name| {
            std::env::var_os(name).is_some_and(|value| !value.is_empty())
        })?;
        Ok(executor)
    }

    fn validate_default_environment(
        binding: &RemoteExecution,
        executor: Self,
        overridden: impl Fn(&str) -> bool,
    ) -> Result<()> {
        let variable = match executor {
            Self::E2b => Some("ONCE_E2B_TEMPLATE"),
            Self::Microsandbox => Some("ONCE_MICROSANDBOX_IMAGE"),
            Self::Local | Self::Daytona | Self::Docker => None,
        };
        if binding.environment.is_none() && variable.is_some_and(overridden) {
            return Err(Error::RemoteProviderConfig {
                provider: binding.provider.clone(),
                message: "declare an explicit environment in the execution binding when overriding the provider default".into(),
            });
        }
        Ok(())
    }

    pub fn resource_domain(self) -> ResourceDomain {
        match self {
            Self::Local | Self::Microsandbox | Self::Docker => ResourceDomain::Host,
            Self::E2b | Self::Daytona => ResourceDomain::Remote,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Microsandbox => "microsandbox",
            Self::E2b => "e2b",
            Self::Daytona => "daytona",
            Self::Docker => "docker",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_resolution_uses_the_binding_not_the_alias() {
        let mut binding = RemoteExecution::provider("test-fleet");
        binding.executor = Some("e2b".to_string());
        assert_eq!(Executor::from_binding(&binding).unwrap(), Executor::E2b);
    }

    #[test]
    fn virtual_machines_share_the_host_domain() {
        assert_eq!(Executor::Local.resource_domain(), ResourceDomain::Host);
        assert_eq!(
            Executor::Microsandbox.resource_domain(),
            ResourceDomain::Host
        );
        assert_eq!(Executor::E2b.resource_domain(), ResourceDomain::Remote);
        assert_eq!(Executor::Daytona.resource_domain(), ResourceDomain::Remote);
    }

    #[test]
    fn unenforced_tenant_requirements_fail_closed() {
        for adapter in ["local", "microsandbox", "e2b", "daytona"] {
            for project in [false, true] {
                let mut binding = RemoteExecution::provider(adapter);
                if project {
                    binding.project = Some("required-project".into());
                } else {
                    binding.account = Some("required-account".into());
                }
                assert!(matches!(
                    Executor::from_binding(&binding),
                    Err(Error::UnsupportedExecutorRequirement { .. })
                ));
            }
        }
    }

    #[test]
    fn implicit_provider_overrides_cannot_reuse_an_unbound_environment() {
        let mut binding = RemoteExecution::provider("e2b");
        assert!(Executor::validate_default_environment(&binding, Executor::E2b, |_| true).is_err());
        binding.environment = Some("bound-template".into());
        assert!(Executor::validate_default_environment(&binding, Executor::E2b, |_| true).is_ok());
        let local = RemoteExecution::provider("local");
        assert!(
            Executor::validate_default_environment(&local, Executor::Local, |_| panic!(
                "native execution has no image"
            ))
            .is_ok()
        );
    }

    #[test]
    fn unknown_adapters_fail_closed() {
        for name in ["unknown", "tuist", ""] {
            assert!(matches!(
                Executor::from_binding(&RemoteExecution::provider(name)),
                Err(Error::UnsupportedRemoteProvider { .. })
            ));
        }
    }
}
