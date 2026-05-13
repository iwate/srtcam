#[derive(Debug)]
pub enum LiveEvent {
    Frame(Vec<u8>),
    StreamUp,
    StreamDown,
}
