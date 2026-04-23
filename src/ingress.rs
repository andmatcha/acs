#[derive(Debug, Clone)]
pub(crate) struct IngressFrame {
    pub input_id: String,
    pub bytes: Vec<u8>,
}
