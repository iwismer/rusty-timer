use crate::workers::Client;

#[allow(non_camel_case_types)]
#[derive(Debug)]
pub enum Message {
    SHUTDOWN,
    CHIP_READ(String),
    CLIENT(Client),
}
