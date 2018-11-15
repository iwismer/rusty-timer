# Rusty Timer

This contains a set of timing related utilities for the Ipico timing system.

This project requires Rust nightly to be built

## Read Streamer

This is a chip read forwarding program designed for race timing. It allows timers to overcome the connection limits on some chip reader systems.

It has been tested on the Ipico Lite reader.

### Building

Run with: ```cargo run --bin streamer -- [args]```
Build with ```cargo build --release --bin streamer```

### Running

    USAGE:
        streamer [OPTIONS] <reader_ip>

    FLAGS:
        -h, --help       Prints help information
        -V, --version    Prints version information

    OPTIONS:
        -f, --file <file>    The file to output the reads to
        -p, --port <port>    The port of the local machine to listen for connections [default: 10001]

    ARGS:
        <reader_ip>    The IP address of the reader to connect to

#### Examples

Stream reads from a reader, leaving the local port assignment to the OS: ```streamer 10.0.0.51```

Stream reads from a reader located at 10.0.0.51, specifying a local port of 10001 ```streamer -p 10001 10.0.0.51```

Stream reads from a reader and save all the reads to a file called reads.txt in the current directory ```streamer -f reads.txt 10.0.0.51```

## Read Emulator

This is a chip read emulation program designed for testing race timing software.

### Building

Run with: ```cargo run --bin emulator -- [args]```
Build with ```cargo build --release --bin emulator```

### Running

    USAGE:
        emulator [OPTIONS]

    FLAGS:
        -h, --help       Prints help information
        -V, --version    Prints version information

    OPTIONS:
        -d, --delay <delay>    Delay between reads [default: 1000]
        -f, --file <file>      The file to get the reads from
        -p, --port <port>      The port of the local machine to listen for connections [default: 10001]

## Read Parser

This is a chip read parsing libarary designed for Ipico chip reads.

### Building

Run with: ```cargo run --bin reads -- [args]```
Build with ```cargo build --release --bin reads```

### Running

This portion does not yet have a usable interface.

## Licence

See LICENCE.txt
