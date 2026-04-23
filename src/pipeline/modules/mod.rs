use crate::input::compact;
use crate::output::OutputFormat;
use crate::pipeline::config::{
    ClassifyModuleConfig, FilterModuleConfig, RouterModuleConfig, TransformChainConfig,
    TransformModuleConfig,
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
    }
}

pub(crate) fn build_router(config: &RouterModuleConfig) -> Box<dyn MessageRouter> {
    match config {
        RouterModuleConfig::Broadcast { outputs } => Box::new(BroadcastRouter {
            outputs: outputs.clone(),
        }),
        RouterModuleConfig::SourceMap {
            routes,
            default_outputs,
        } => Box::new(SourceMapRouter {
            routes: routes.clone(),
            default_outputs: default_outputs.clone(),
        }),
    }
}

fn build_transform(
    config: &TransformModuleConfig,
    _pipeline_inputs: &[String],
) -> Result<Box<dyn MessageTransform>, String> {
    match config {
        TransformModuleConfig::Identity => Ok(Box::new(IdentityTransform)),
        TransformModuleConfig::Ds4ToCompact => Ok(Box::new(Ds4ToCompactTransform)),
        TransformModuleConfig::OutputEncode { format } => Ok(Box::new(OutputEncodeTransform {
            format: *format,
            driver: format.create_driver()?,
        })),
    }
}

struct AllowAllFilter;

impl FrameFilter for AllowAllFilter {
    fn accept(&mut self, _: &IngressFrame) -> Result<bool, String> {
        Ok(true)
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

struct NoopClassifier;

impl MessageClassifier for NoopClassifier {
    fn classify(&mut self, _: &mut RouteMessage) -> Result<(), String> {
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
