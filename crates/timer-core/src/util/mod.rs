#![allow(dead_code)]
use std::fs::{File, remove_file};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;
use tokio::signal;

pub mod io;

pub async fn signal_handler() {
    signal::ctrl_c().await.unwrap();
}

/// Check if the string is a valid IPv4 address
pub fn is_ip_addr(ip: String) -> Result<(), String> {
    match ip.parse::<Ipv4Addr>() {
        Ok(_) => Ok(()),
        Err(_) => Err("Invalid IP Address".to_owned()),
    }
}

/// Check if the string is a valid IPv4 socket address
pub fn is_socket_addr(socket: String) -> Result<(), String> {
    match socket.parse::<SocketAddrV4>() {
        Ok(_) => Ok(()),
        Err(_) => Err("Invalid Socket Address".to_owned()),
    }
}

/// Check if the string is a valid port
pub fn is_port(port: String) -> Result<(), String> {
    match port.parse::<u16>() {
        Ok(_) => Ok(()),
        Err(_) => Err("Invalid port number".to_owned()),
    }
}

/// Check that the path does not already point to a file
pub fn is_empty_path(path_str: String) -> Result<(), String> {
    let path = Path::new(&path_str);
    match path.exists() {
        true => Err("File exists on file system! Use a different file".to_owned()),
        false => {
            // Check that the file can be created
            match File::create(path) {
                Ok(_) => {
                    remove_file(path).unwrap_or(());
                    Ok(())
                }
                Err(_) => Err("File path invalid! Use a different file".to_owned()),
            }
        }
    }
}

/// Check that the file exists
pub fn is_file(file_str: String) -> Result<(), String> {
    let path = Path::new(&file_str);
    match path.is_file() {
        true => Ok(()),
        false => Err("File doesn't exists on file system! Use a different file".to_owned()),
    }
}

pub fn is_delay(delay: String) -> Result<(), String> {
    match delay.parse::<u32>() {
        Ok(_) => Ok(()),
        Err(_) => Err("Invalid delay value".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_ip_addr() {
        assert!(is_ip_addr("1.1.1.1".to_owned()).is_ok());
        assert!(is_ip_addr("192.168.1.1".to_owned()).is_ok());
        assert!(is_ip_addr("255.255.255.255".to_owned()).is_ok());
        assert!(is_ip_addr("0.0.0.0".to_owned()).is_ok());
        assert!(is_ip_addr("10.0.0.1".to_owned()).is_ok());

        assert!(is_ip_addr("-1.-1.-.-1".to_owned()).is_err());
        assert!(is_ip_addr("foobar".to_owned()).is_err());
        assert!(is_ip_addr("1.1.1".to_owned()).is_err());
        assert!(is_ip_addr("1.1.1.1.1".to_owned()).is_err());
        assert!(is_ip_addr("1.1.1.1:8080".to_owned()).is_err());
        assert!(is_ip_addr("".to_owned()).is_err());
    }

    #[test]
    fn test_is_socket_addr() {
        assert!(is_socket_addr("1.1.1.1:1".to_owned()).is_ok());
        assert!(is_socket_addr("192.168.1.1:8080".to_owned()).is_ok());
        assert!(is_socket_addr("255.255.255.255:60000".to_owned()).is_ok());
        assert!(is_socket_addr("0.0.0.0:0".to_owned()).is_ok());
        assert!(is_socket_addr("10.0.0.1:10000".to_owned()).is_ok());

        assert!(is_socket_addr("1.1.1.1".to_owned()).is_err());
        assert!(is_socket_addr("foobar".to_owned()).is_err());
        assert!(is_socket_addr("1.1.1".to_owned()).is_err());
        assert!(is_socket_addr("1.1.1.1.1".to_owned()).is_err());
        assert!(is_socket_addr("1.1.1.1:-1".to_owned()).is_err());
        assert!(is_socket_addr("1.1.1.1:100000000".to_owned()).is_err());
        assert!(is_socket_addr("".to_owned()).is_err());
    }

    #[test]
    fn test_is_port() {
        assert!(is_port("1".to_owned()).is_ok());
        assert!(is_port("8080".to_owned()).is_ok());
        assert!(is_port("60000".to_owned()).is_ok());
        assert!(is_port("0".to_owned()).is_ok());
        assert!(is_port("10000".to_owned()).is_ok());

        assert!(is_port("-1".to_owned()).is_err());
        assert!(is_port("foobar".to_owned()).is_err());
        assert!(is_port("100000000".to_owned()).is_err());
        assert!(is_port("".to_owned()).is_err());
    }

    #[test]
    fn test_is_empty_path() {
        assert!(is_empty_path("qwerty.txt".to_owned()).is_ok());

        assert!(is_empty_path("".to_owned()).is_err());
        assert!(is_empty_path("test_assets/bibchip/empty.txt".to_owned()).is_err());
        assert!(is_empty_path("test_assets/bibchip".to_owned()).is_err());
    }

    #[test]
    fn test_is_file() {
        assert!(is_file("test_assets/bibchip/single.txt".to_owned()).is_ok());

        assert!(is_file("".to_owned()).is_err());
        assert!(is_file("qwety.txt".to_owned()).is_err());
        assert!(is_file("test_assets/bibchip".to_owned()).is_err());
    }

    #[test]
    fn test_is_delay() {
        assert!(is_delay("1".to_owned()).is_ok());
        assert!(is_delay("8080".to_owned()).is_ok());
        assert!(is_delay("60000".to_owned()).is_ok());
        assert!(is_delay("0".to_owned()).is_ok());
        assert!(is_delay("10000".to_owned()).is_ok());

        assert!(is_delay("-1".to_owned()).is_err());
        assert!(is_delay("foobar".to_owned()).is_err());
        assert!(is_delay("".to_owned()).is_err());
    }
}
