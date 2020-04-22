use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;

pub mod io;

/// Check if the string is a valid IPv4 address
pub fn is_ip_addr(ip: String) -> Result<(), String> {
    match ip.parse::<Ipv4Addr>() {
        Ok(_) => Ok(()),
        Err(_) => Err("Invalid IP Address".to_string()),
    }
}

/// Check if the string is a valid IPv4 socket address
pub fn is_socket_addr(socket: String) -> Result<(), String> {
    match socket.parse::<SocketAddrV4>() {
        Ok(_) => Ok(()),
        Err(_) => Err("Invalid Socket Address".to_string()),
    }
}

/// Check if the string is a valid port
pub fn is_port(port: String) -> Result<(), String> {
    match port.parse::<u16>() {
        Ok(_) => Ok(()),
        Err(_) => Err("Invalid port number".to_string()),
    }
}

/// Check that the path does not already point to a file
pub fn is_path(path_str: String) -> Result<(), String> {
    let path = Path::new(&path_str);
    match path.exists() {
        true => Err("File exists on file system! Use a different file".to_string()),
        false => Ok(()),
    }
}

/// Check that the path does not already point to a file
pub fn is_file(file_str: String) -> Result<(), String> {
    let path = Path::new(&file_str);
    match path.exists() {
        true => Ok(()),
        false => Err("File doesn't exists on file system! Use a different file".to_string()),
    }
}
