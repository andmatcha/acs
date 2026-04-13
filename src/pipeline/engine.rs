use crate::pipeline::config::PipelineSpec;
use crate::pipeline::message::{DispatchPlan, RouteMessage};
use crate::pipeline::modules::{
    build_classifier, build_filter, build_router, build_transform_chain,
};
use crate::session::event::IngressFrame;

pub(crate) struct PipelineEngine {
    pipelines: Vec<PipelineInstance>,
}

struct PipelineInstance {
    id: String,
    inputs: Vec<String>,
    filter: Box<dyn FrameFilter>,
    transforms: Vec<Box<dyn MessageTransform>>,
    classifier: Box<dyn MessageClassifier>,
    router: Box<dyn MessageRouter>,
}

pub(crate) trait FrameFilter {
    fn accept(&mut self, frame: &IngressFrame) -> Result<bool, String>;
}

pub(crate) trait MessageTransform {
    fn transform(&mut self, message: RouteMessage) -> Result<Vec<RouteMessage>, String>;
}

pub(crate) trait MessageClassifier {
    fn classify(&mut self, message: &mut RouteMessage) -> Result<(), String>;
}

pub(crate) trait MessageRouter {
    fn route(&mut self, message: &RouteMessage) -> Result<Vec<String>, String>;
}

impl PipelineEngine {
    pub(crate) fn new(spec: &PipelineSpec) -> Result<Self, String> {
        let pipelines = spec
            .pipelines
            .iter()
            .map(|definition| {
                Ok(PipelineInstance {
                    id: definition.id.clone(),
                    inputs: definition.inputs.clone(),
                    filter: build_filter(&definition.filter),
                    transforms: build_transform_chain(&definition.transform, &definition.inputs)?,
                    classifier: build_classifier(&definition.classify),
                    router: build_router(&definition.router),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        Ok(Self { pipelines })
    }

    pub(crate) fn process_frame(&mut self, frame: &IngressFrame) -> Result<Vec<DispatchPlan>, String> {
        let mut dispatches = Vec::new();

        for pipeline in &mut self.pipelines {
            if !pipeline.inputs.iter().any(|input| input == &frame.input_id) {
                continue;
            }
            if !pipeline.filter.accept(frame)? {
                continue;
            }

            let mut messages = vec![RouteMessage::from_frame(&pipeline.id, frame)];
            for transform in &mut pipeline.transforms {
                let mut next_messages = Vec::new();
                for message in messages {
                    next_messages.extend(transform.transform(message)?);
                }
                messages = next_messages;
                if messages.is_empty() {
                    break;
                }
            }

            for mut message in messages {
                pipeline.classifier.classify(&mut message)?;
                let outputs = pipeline.router.route(&message)?;
                for output_id in outputs {
                    dispatches.push(DispatchPlan {
                        output_id,
                        bytes: message.payload.clone(),
                    });
                }
            }
        }

        Ok(dispatches)
    }
}

pub(crate) mod traits {
    pub(crate) use super::{FrameFilter, MessageClassifier, MessageRouter, MessageTransform};
}

#[cfg(test)]
mod tests {
    use super::PipelineEngine;
    use crate::pipeline::{
        ClassifyModuleConfig, FilterModuleConfig, PipelineDefinition, PipelineSpec,
        RouterModuleConfig, TransformChainConfig, TransformModuleConfig,
    };
    use crate::session::event::IngressFrame;

    #[test]
    fn identity_broadcast_routes_bytes_to_target() {
        let spec = PipelineSpec {
            pipelines: vec![PipelineDefinition {
                id: String::from("p1"),
                inputs: vec![String::from("in_a")],
                filter: FilterModuleConfig::AllowAll,
                transform: TransformChainConfig {
                    modules: vec![TransformModuleConfig::Identity],
                },
                classify: ClassifyModuleConfig::None,
                router: RouterModuleConfig::Broadcast {
                    outputs: vec![String::from("out_main")],
                },
            }],
        };
        let mut engine = PipelineEngine::new(&spec).unwrap();

        let dispatches = engine
            .process_frame(&IngressFrame {
                input_id: String::from("in_a"),
                port: String::from("/dev/ttyUSB0"),
                bytes: b"abc".to_vec(),
                sequence: 1,
            })
            .unwrap();

        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].output_id, "out_main");
        assert_eq!(dispatches[0].bytes, b"abc");
    }
}
