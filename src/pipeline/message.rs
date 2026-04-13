use crate::session::event::IngressFrame;

#[derive(Debug, Clone)]
pub(crate) struct RouteMessage {
    pub source_input_ids: Vec<String>,
    pub payload: Vec<u8>,
    pub tags: Vec<String>,
}

impl RouteMessage {
    pub(crate) fn from_frame(frame: &IngressFrame) -> Self {
        Self {
            source_input_ids: vec![frame.input_id.clone()],
            payload: frame.bytes.clone(),
            tags: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DispatchPlan {
    pub output_id: String,
    pub bytes: Vec<u8>,
}
