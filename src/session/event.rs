#[derive(Debug, Clone)]
pub(crate) struct IngressFrame {
    pub input_id: String,
    pub port: String,
    pub bytes: Vec<u8>,
    pub sequence: u64,
}

#[derive(Debug, Clone)]
pub(crate) enum SessionEvent {
    InputData {
        input_id: String,
        port: String,
        bytes: Vec<u8>,
    },
    InputError {
        input_id: String,
        port: String,
        message: String,
    },
}
