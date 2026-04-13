use crate::session::event::IngressFrame;
use std::collections::BTreeMap;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct RouteMessage {
    pub pipeline_id: String,
    pub source_input_ids: Vec<String>,
    pub payload: Vec<u8>,
    pub sequence: u64,
    pub tags: Vec<String>,
    pub attributes: BTreeMap<String, String>,
}

impl RouteMessage {
    pub(crate) fn from_frame(pipeline_id: &str, frame: &IngressFrame) -> Self {
        let mut attributes = BTreeMap::new();
        attributes.insert(String::from("input_id"), frame.input_id.clone());
        attributes.insert(String::from("port"), frame.port.clone());

        Self {
            pipeline_id: pipeline_id.to_owned(),
            source_input_ids: vec![frame.input_id.clone()],
            payload: frame.bytes.clone(),
            sequence: frame.sequence,
            tags: Vec::new(),
            attributes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DispatchPlan {
    pub output_id: String,
    pub bytes: Vec<u8>,
}
