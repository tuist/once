pub fn app_bundle_path(package: &str, name: &str) -> String {
    if package.is_empty() {
        format!(".fabrik/out/{name}.app")
    } else {
        format!(".fabrik/out/{package}/{name}.app")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppleKind {
    IosApp,
    SwiftLibrary,
    AppleStaticFramework,
    AppleDynamicFramework,
    MacosCommandLineApplication,
}

impl AppleKind {
    pub fn parse(kind: &str) -> Option<Self> {
        match kind {
            "apple_ios_app" => Some(Self::IosApp),
            "swift_library" => Some(Self::SwiftLibrary),
            "apple_static_framework" => Some(Self::AppleStaticFramework),
            "apple_dynamic_framework" => Some(Self::AppleDynamicFramework),
            "macos_command_line_application" => Some(Self::MacosCommandLineApplication),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IosApp => "apple_ios_app",
            Self::SwiftLibrary => "swift_library",
            Self::AppleStaticFramework => "apple_static_framework",
            Self::AppleDynamicFramework => "apple_dynamic_framework",
            Self::MacosCommandLineApplication => "macos_command_line_application",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SwiftArtifact {
    pub module_name: String,
    pub import_search: SwiftImportSearch,
    pub import_searches: Vec<SwiftImportSearch>,
    pub link_inputs: Vec<SwiftLinkInput>,
    pub output: String,
    pub action_digest: fabrik_cas::Digest,
    pub kind: AppleKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwiftImportSearch {
    ModuleDir(String),
    FrameworkParent(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwiftLinkInput {
    StaticArchive(String),
    StaticFramework { name: String, parent: String },
    DynamicFramework { name: String, parent: String },
}

impl SwiftArtifact {
    pub fn import_args(&self) -> Vec<String> {
        self.import_searches
            .iter()
            .flat_map(SwiftImportSearch::args)
            .collect()
    }
}

impl SwiftImportSearch {
    pub fn args(&self) -> Vec<String> {
        match self {
            Self::ModuleDir(dir) => vec!["-I".to_string(), dir.clone()],
            Self::FrameworkParent(parent) => vec!["-F".to_string(), parent.clone()],
        }
    }

    pub fn cache_key(&self) -> String {
        match self {
            Self::ModuleDir(dir) => format!("module-dir:{dir}"),
            Self::FrameworkParent(parent) => format!("framework-parent:{parent}"),
        }
    }
}

impl SwiftLinkInput {
    pub fn link_args(&self) -> Vec<String> {
        match self {
            Self::StaticArchive(path) => vec![path.clone()],
            Self::StaticFramework { name, parent } | Self::DynamicFramework { name, parent } => {
                vec![
                    "-F".to_string(),
                    parent.clone(),
                    "-framework".to_string(),
                    name.clone(),
                ]
            }
        }
    }

    pub fn cache_key(&self) -> String {
        match self {
            Self::StaticArchive(path) => format!("static-archive:{path}"),
            Self::StaticFramework { name, parent } => {
                format!("static-framework:{parent}:{name}")
            }
            Self::DynamicFramework { name, parent } => {
                format!("dynamic-framework:{parent}:{name}")
            }
        }
    }
}

pub fn swift_out_dir(package: &str, name: &str) -> String {
    if package.is_empty() {
        format!(".fabrik/out/{name}")
    } else {
        format!(".fabrik/out/{package}/{name}")
    }
}

pub fn swift_static_library_path(out_dir: &str, module_name: &str) -> String {
    format!("{out_dir}/lib{module_name}.a")
}

pub fn executable_path(package: &str, name: &str) -> String {
    if package.is_empty() {
        format!(".fabrik/out/{name}{}", std::env::consts::EXE_SUFFIX)
    } else {
        format!(
            ".fabrik/out/{package}/{name}{}",
            std::env::consts::EXE_SUFFIX
        )
    }
}

pub fn framework_path(package: &str, name: &str) -> String {
    if package.is_empty() {
        format!(".fabrik/out/{name}.framework")
    } else {
        format!(".fabrik/out/{package}/{name}.framework")
    }
}

pub fn parent_dir(path: &str) -> String {
    path.rsplit_once('/')
        .map_or_else(|| ".".to_string(), |(parent, _)| parent.to_string())
}
