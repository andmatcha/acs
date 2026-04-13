use crate::common::extend_unique_strings;
use crate::input::compact;
use crate::output::OutputFormat;
use crate::pipeline::config::{
    ClassifyModuleConfig, FilterModuleConfig, RouterModuleConfig, TagRoutingRule,
    TransformChainConfig, TransformModuleConfig,
};
use crate::pipeline::engine::traits::{
    FrameFilter, MessageClassifier, MessageRouter, MessageTransform,
};
use crate::pipeline::message::RouteMessage;
use crate::session::event::IngressFrame;
use std::collections::BTreeMap;

pub(crate) fn build_filter(config: &FilterModuleConfig) -> Box<dyn FrameFilter> {
    match config {
        FilterModuleConfig::AllowAll => Box::new(AllowAllFilter),
        FilterModuleConfig::DropEmpty => Box::new(DropEmptyFilter),
        FilterModuleConfig::MatchSource { input_ids } => Box::new(MatchSourceFilter {
            input_ids: input_ids.clone(),
        }),
        FilterModuleConfig::MatchPrefix { prefix } => Box::new(MatchPrefixFilter {
            prefix: prefix.clone(),
        }),
    }
}

pub(crate) fn build_transform_chain(
    config: &TransformChainConfig,
    pipeline_inputs: &[String],
) -> Result<Vec<Box<dyn MessageTransform>>, String> {
    config
        .modules
        .iter()
        .map(|module| build_transform(module, pipeline_inputs))
        .collect()
}

pub(crate) fn build_classifier(config: &ClassifyModuleConfig) -> Box<dyn MessageClassifier> {
    match config {
        ClassifyModuleConfig::None => Box::new(NoopClassifier),
        ClassifyModuleConfig::BySource => Box::new(BySourceClassifier),
        ClassifyModuleConfig::TagStatic { tags } => {
            Box::new(StaticTagClassifier { tags: tags.clone() })
        }
        ClassifyModuleConfig::MatchPrefix { prefix, tag } => Box::new(MatchPrefixClassifier {
            prefix: prefix.clone(),
            tag: tag.clone(),
        }),
    }
}

pub(crate) fn build_router(config: &RouterModuleConfig) -> Box<dyn MessageRouter> {
    match config {
        RouterModuleConfig::Broadcast { outputs } => Box::new(BroadcastRouter {
            outputs: outputs.clone(),
        }),
        RouterModuleConfig::RoundRobin { outputs } => Box::new(RoundRobinRouter {
            outputs: outputs.clone(),
            next_index: 0,
        }),
        RouterModuleConfig::SourceMap {
            routes,
            default_outputs,
        } => Box::new(SourceMapRouter {
            routes: routes.clone(),
            default_outputs: default_outputs.clone(),
        }),
        RouterModuleConfig::TagBased {
            routes,
            default_outputs,
        } => Box::new(TagBasedRouter {
            routes: routes.clone(),
            default_outputs: default_outputs.clone(),
        }),
    }
}

fn build_transform(
    config: &TransformModuleConfig,
    pipeline_inputs: &[String],
) -> Result<Box<dyn MessageTransform>, String> {
    match config {
        TransformModuleConfig::Identity => Ok(Box::new(IdentityTransform)),
        TransformModuleConfig::Ds4ToCompact => Ok(Box::new(Ds4ToCompactTransform)),
        TransformModuleConfig::OutputEncode { format } => Ok(Box::new(OutputEncodeTransform {
            format: *format,
            driver: format.create_driver(),
        })),
        TransformModuleConfig::JoinLatest {
            separator,
            require_all,
        } => Ok(Box::new(JoinLatestTransform {
            input_order: pipeline_inputs.to_vec(),
            latest_by_input: BTreeMap::new(),
            separator: separator.clone(),
            require_all: *require_all,
        })),
    }
}

struct AllowAllFilter;

impl FrameFilter for AllowAllFilter {
    fn accept(&mut self, _: &IngressFrame) -> Result<bool, String> {
        Ok(true)
    }
}

struct DropEmptyFilter;

impl FrameFilter for DropEmptyFilter {
    fn accept(&mut self, frame: &IngressFrame) -> Result<bool, String> {
        Ok(!frame.bytes.is_empty())
    }
}

struct MatchSourceFilter {
    input_ids: Vec<String>,
}

impl FrameFilter for MatchSourceFilter {
    fn accept(&mut self, frame: &IngressFrame) -> Result<bool, String> {
        Ok(self.input_ids.iter().any(|input| input == &frame.input_id))
    }
}

struct MatchPrefixFilter {
    prefix: Vec<u8>,
}

impl FrameFilter for MatchPrefixFilter {
    fn accept(&mut self, frame: &IngressFrame) -> Result<bool, String> {
        Ok(frame.bytes.starts_with(&self.prefix))
    }
}

struct IdentityTransform;

impl MessageTransform for IdentityTransform {
    fn transform(&mut self, message: RouteMessage) -> Result<Vec<RouteMessage>, String> {
        Ok(vec![message])
    }
}

struct Ds4ToCompactTransform;

impl MessageTransform for Ds4ToCompactTransform {
    fn transform(&mut self, mut message: RouteMessage) -> Result<Vec<RouteMessage>, String> {
        let compact = compact::convert_input_report(&message.payload)
            .map_err(|error| format!("failed to convert DS4 report: {error}"))?;
        message.payload = compact.to_vec();
        Ok(vec![message])
    }
}

struct OutputEncodeTransform {
    format: OutputFormat,
    driver: Box<dyn crate::output::formats::OutputDriver>,
}

impl MessageTransform for OutputEncodeTransform {
    fn transform(&mut self, mut message: RouteMessage) -> Result<Vec<RouteMessage>, String> {
        let compact_report: [u8; 8] = message.payload.as_slice().try_into().map_err(|_| {
            format!(
                "output_encode({}) expects an 8-byte compact report",
                self.format.as_str()
            )
        })?;
        message.payload = self.driver.encode(&compact_report)?;
        Ok(vec![message])
    }
}

struct JoinLatestTransform {
    input_order: Vec<String>,
    latest_by_input: BTreeMap<String, Vec<u8>>,
    separator: Vec<u8>,
    require_all: bool,
}

impl MessageTransform for JoinLatestTransform {
    fn transform(&mut self, message: RouteMessage) -> Result<Vec<RouteMessage>, String> {
        if let Some(input_id) = message.source_input_ids.first() {
            self.latest_by_input
                .insert(input_id.clone(), message.payload.clone());
        }

        if self.require_all
            && self
                .input_order
                .iter()
                .any(|input| !self.latest_by_input.contains_key(input))
        {
            return Ok(Vec::new());
        }

        let mut payload = Vec::new();
        let mut sources = Vec::new();
        for input_id in &self.input_order {
            let Some(bytes) = self.latest_by_input.get(input_id) else {
                continue;
            };
            if !payload.is_empty() {
                payload.extend_from_slice(&self.separator);
            }
            payload.extend_from_slice(bytes);
            sources.push(input_id.clone());
        }

        if payload.is_empty() {
            return Ok(Vec::new());
        }

        let mut joined = message;
        joined.payload = payload;
        joined.source_input_ids = sources;
        Ok(vec![joined])
    }
}

struct NoopClassifier;

impl MessageClassifier for NoopClassifier {
    fn classify(&mut self, _: &mut RouteMessage) -> Result<(), String> {
        Ok(())
    }
}

struct BySourceClassifier;

impl MessageClassifier for BySourceClassifier {
    fn classify(&mut self, message: &mut RouteMessage) -> Result<(), String> {
        for input_id in &message.source_input_ids {
            let tag = format!("source:{input_id}");
            if !message.tags.iter().any(|existing| existing == &tag) {
                message.tags.push(tag);
            }
        }
        Ok(())
    }
}

struct StaticTagClassifier {
    tags: Vec<String>,
}

impl MessageClassifier for StaticTagClassifier {
    fn classify(&mut self, message: &mut RouteMessage) -> Result<(), String> {
        for tag in &self.tags {
            if !message.tags.iter().any(|existing| existing == tag) {
                message.tags.push(tag.clone());
            }
        }
        Ok(())
    }
}

struct MatchPrefixClassifier {
    prefix: Vec<u8>,
    tag: String,
}

impl MessageClassifier for MatchPrefixClassifier {
    fn classify(&mut self, message: &mut RouteMessage) -> Result<(), String> {
        if message.payload.starts_with(&self.prefix)
            && !message.tags.iter().any(|existing| existing == &self.tag)
        {
            message.tags.push(self.tag.clone());
        }
        Ok(())
    }
}

struct BroadcastRouter {
    outputs: Vec<String>,
}

impl MessageRouter for BroadcastRouter {
    fn route(&mut self, _: &RouteMessage) -> Result<Vec<String>, String> {
        Ok(self.outputs.clone())
    }
}

struct RoundRobinRouter {
    outputs: Vec<String>,
    next_index: usize,
}

impl MessageRouter for RoundRobinRouter {
    fn route(&mut self, _: &RouteMessage) -> Result<Vec<String>, String> {
        if self.outputs.is_empty() {
            return Ok(Vec::new());
        }

        let output = self.outputs[self.next_index % self.outputs.len()].clone();
        self.next_index = (self.next_index + 1) % self.outputs.len();
        Ok(vec![output])
    }
}

struct SourceMapRouter {
    routes: BTreeMap<String, Vec<String>>,
    default_outputs: Vec<String>,
}

impl MessageRouter for SourceMapRouter {
    fn route(&mut self, message: &RouteMessage) -> Result<Vec<String>, String> {
        for input_id in &message.source_input_ids {
            if let Some(outputs) = self.routes.get(input_id) {
                return Ok(outputs.clone());
            }
        }

        Ok(self.default_outputs.clone())
    }
}

struct TagBasedRouter {
    routes: Vec<TagRoutingRule>,
    default_outputs: Vec<String>,
}

impl MessageRouter for TagBasedRouter {
    fn route(&mut self, message: &RouteMessage) -> Result<Vec<String>, String> {
        let mut outputs = Vec::new();
        for rule in &self.routes {
            if message.tags.iter().any(|tag| tag == &rule.tag) {
                extend_unique_strings(&mut outputs, &rule.outputs);
            }
        }

        if outputs.is_empty() {
            extend_unique_strings(&mut outputs, &self.default_outputs);
        }

        Ok(outputs)
    }
}
