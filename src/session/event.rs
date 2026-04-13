#[derive(Debug, Clone)]
pub(crate) struct IngressFrame {
    pub input_id: String,
    pub bytes: Vec<u8>,
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
