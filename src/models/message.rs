use crate::workers::Client;

#[derive(Debug)]
pub enum Message {
    SHUTDOWN,
    CHIP_READ(String),
    CLIENT(Client),
}
