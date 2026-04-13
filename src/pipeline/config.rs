use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub(crate) struct PipelineSpec {
    pub pipelines: Vec<PipelineDefinition>,
}

#[derive(Debug, Clone)]
pub(crate) struct PipelineDefinition {
    pub id: String,
    pub inputs: Vec<String>,
    pub filter: FilterModuleConfig,
    pub transform: TransformChainConfig,
    pub classify: ClassifyModuleConfig,
    pub router: RouterModuleConfig,
}

#[derive(Debug, Clone)]
pub(crate) enum FilterModuleConfig {
    AllowAll,
    DropEmpty,
    MatchSource { input_ids: Vec<String> },
    MatchPrefix { prefix: Vec<u8> },
}

#[derive(Debug, Clone)]
pub(crate) struct TransformChainConfig {
    pub modules: Vec<TransformModuleConfig>,
}

#[derive(Debug, Clone)]
pub(crate) enum TransformModuleConfig {
    Identity,
    Ds4ToCompact,
    Arm9Encode,
    JoinLatest { separator: Vec<u8>, require_all: bool },
}

#[derive(Debug, Clone)]
pub(crate) enum ClassifyModuleConfig {
    None,
    BySource,
    TagStatic { tags: Vec<String> },
    MatchPrefix { prefix: Vec<u8>, tag: String },
}

#[derive(Debug, Clone)]
pub(crate) enum RouterModuleConfig {
    Broadcast { outputs: Vec<String> },
    RoundRobin { outputs: Vec<String> },
    SourceMap {
        routes: BTreeMap<String, Vec<String>>,
        default_outputs: Vec<String>,
    },
    TagBased {
        routes: Vec<TagRoutingRule>,
        default_outputs: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct TagRoutingRule {
    pub tag: String,
    pub outputs: Vec<String>,
}

impl Default for FilterModuleConfig {
    fn default() -> Self {
        Self::AllowAll
    }
}

impl Default for TransformChainConfig {
    fn default() -> Self {
        Self {
            modules: vec![TransformModuleConfig::Identity],
        }
    }
}

impl Default for ClassifyModuleConfig {
    fn default() -> Self {
        Self::None
    }
}
