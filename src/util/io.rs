use encoding::all::WINDOWS_1252;
use encoding::{DecoderTrap, Encoding};
use std::path::Path;

/// Reads a file into a vec of Strings
/// First try reading UTF-8 encoding, if that doesn't work, then read as a
/// WIN1252 encoded file
pub fn read_file(path_str: &String) -> Result<Vec<String>, String> {
    let path = Path::new(path_str);
    let buffer = match std::fs::read(path) {
        Err(desc) => {
            return Err(format!(
                "couldn't read {}: {}",
                path.display(),
                desc.to_string()
            ))
        }
        Ok(buf) => buf,
    };
    match std::str::from_utf8(&buffer) {
        Err(_desc) => match WINDOWS_1252.decode(buffer.as_slice(), DecoderTrap::Replace) {
            Err(desc) => Err(format!("couldn't read {}: {}", path.display(), desc)),
            Ok(s) => Ok(s.to_string()),
        },
        Ok(s) => Ok(s.to_string()),
    }
    .map(|s| s.split('\n').map(|s| s.to_string()).collect())
}
