use crate::output::OutputFormat;
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

#[derive(Debug, Clone, Default)]
pub(crate) enum FilterModuleConfig {
    #[default]
    AllowAll,
}

#[derive(Debug, Clone)]
pub(crate) struct TransformChainConfig {
    pub modules: Vec<TransformModuleConfig>,
}

#[derive(Debug, Clone)]
pub(crate) enum TransformModuleConfig {
    Identity,
    Ds4ToCompact,
    OutputEncode { format: OutputFormat },
}

#[derive(Debug, Clone, Default)]
pub(crate) enum ClassifyModuleConfig {
    #[default]
    None,
}

#[derive(Debug, Clone)]
pub(crate) enum RouterModuleConfig {
    Broadcast {
        outputs: Vec<String>,
    },
    SourceMap {
        routes: BTreeMap<String, Vec<String>>,
        default_outputs: Vec<String>,
    },
}

impl Default for TransformChainConfig {
    fn default() -> Self {
        Self {
            modules: vec![TransformModuleConfig::Identity],
        }
    }
}
