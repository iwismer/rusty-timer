# Rusty Timer

[![Build Status](https://travis-ci.org/iwismer/rusty-timer.svg?branch=master)](https://travis-ci.org/iwismer/rusty-timer)

This contains a set of timing related utilities for the Ipico timing system.

## Read Streamer

This is a chip read forwarding program designed for race timing. It allows timers to overcome the connection limits on some chip reader systems.
This program connects to a single reader, and forwards all the reads to any connected listening programs. There is no theoretical limit to the number of connected clients, but it has not been tested with more than 4 at a time. This program will also save all the collected reads to a file for backup.

It has been tested on the Ipico Lite reader.

### Building

Run with: ```cargo run --bin streamer -- [args]```
Build with ```cargo build --release --bin streamer```

### Running

    USAGE:
    streamer [FLAGS] [OPTIONS] <reader_ip>

    FLAGS:
        -h, --help        Prints help information
        -B, --buffer      Buffer the output. Use if high CPU use in encountered
        -V, --version     Prints version information

    OPTIONS:
        -b, --bibchip <bibchip>            The bib-chip file
        -f, --file <file>                  The file to output the reads to
        -P, --ppl <participants>           The .ppl participant file
        -p, --port <port>                  The port of the local machine to bind to [default: 10001]
        -r, --reader-port <reader-port>    The port of the reader to connect to [default: 10000]

    ARGS:
        <reader_ip>    The IP address of the reader to connect to

#### Examples

Stream reads from a reader, leaving the local port assignment to the OS: ```streamer 10.0.0.51```

Stream reads from a reader located at 10.0.0.51, specifying a local port of 10005 ```streamer -p 10005 10.0.0.51```

Stream reads from a reader and save all the reads to a file called reads.txt in the current directory ```streamer -f reads.txt 10.0.0.51```

### TODO

- Better documentation
- Basic GUI
- Read checksum validation
- Tests

## Read Emulator

This is a chip read emulation program designed for testing race timing software. I generates random, valid reads (except the checksum).

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

## Licence

GPL3

See LICENCE.txt
