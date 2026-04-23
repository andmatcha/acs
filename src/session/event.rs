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
