use crate::ingress::IngressFrame;

#[derive(Debug, Clone)]
pub(crate) struct RouteMessage {
    pub source_input_ids: Vec<String>,
    pub payload: Vec<u8>,
}

impl RouteMessage {
    pub(crate) fn from_frame(frame: &IngressFrame) -> Self {
        Self {
            source_input_ids: vec![frame.input_id.clone()],
            payload: frame.bytes.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DispatchPlan {
    pub output_id: String,
    pub bytes: Vec<u8>,
}
