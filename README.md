# Rusty Timer

[![Build Status](https://travis-ci.org/iwismer/rusty-timer.svg?branch=master)](https://travis-ci.org/iwismer/rusty-timer)

This contains a set of timing related utilities for the Ipico timing system.

## Read Streamer

This is a chip read forwarding program designed for race timing. It allows timers to overcome the connection limits on some chip reader systems.
This program connects to one or more readers, and forwards all the reads to any connected listening programs. There is no theoretical limit to the number of connected clients, but it has not been tested with more than 4 at a time. This program will also save all the collected reads to a file for backup.

### Features

- Multiple reader connections
- Multiple client connections
- Automatic reader reconnection on disconnect
- Save reads to a file
- Display participant information for each read
- Performant: uses less than 1MB of memory, handles at least 1000 reads/second

### Building

Run directly with: ```cargo run --bin streamer -- [args]```

Build with ```cargo build --release --bin streamer```

### Running

    USAGE:
        streamer.exe [FLAGS] [OPTIONS] <reader_ip>...

    FLAGS:
        -h, --help       Prints help information
        -B, --buffer     Buffer the output. Use if high CPU use in encountered
        -V, --version    Prints version information

    OPTIONS:
        -b, --bibchip <bibchip>     The bib-chip file
        -f, --file <file>           The file to output the reads to
        -P, --ppl <participants>    The .ppl participant file
        -p, --port <port>           The port of the local machine to bind to [default: 10001]
        -t, --type <read_type>      The type of read the reader is sending [default: raw]  [possible values: raw, fsls]

    ARGS:
        <reader_ip>...    The socket address of the reader to connect to. Eg. 192.168.0.52:10000

#### Examples

In Windows, replace `streamer` with `streamer.exe` (and include the path to it, if it's not in the current directory). In linux use `./streamer`.

Stream reads from a reader, leaving the local port assignment to the OS: ```streamer 10.0.0.51:10000```

Stream reads from 2 readers, using a local port of 10003: ```streamer 10.0.0.51:10000 10.0.0.52:10000 -p 10003```

Stream reads from a reader located at 10.0.0.51, specifying a local port of 10005 ```streamer -p 10005 10.0.0.51:10000```

Stream reads from a reader and save all the reads to a file called reads.txt in the current directory ```streamer -f reads.txt 10.0.0.51:10000```

### TODO

- Better documentation
- Basic GUI
- Make compatible with binary records

## Read Emulator

This is a chip read emulation program designed for testing race timing software. It generates valid reads that are all the same chip, but use the current time. You can also use a reads file as input, and it will simply send one read after each delay cycle.

This can be used to test the read streaming program.

### Building

Run with: ```cargo run --bin emulator -- [args]```

Build with ```cargo build --release --bin emulator```

### Running

    USAGE:
        emulator.exe [OPTIONS]

    FLAGS:
        -h, --help       Prints help information
        -V, --version    Prints version information

    OPTIONS:
        -d, --delay <delay>       Delay between reads [default: 1000]
        -f, --file <file>         The file to get the reads from
        -p, --port <port>         The port of the local machine to listen for connections [default: 10001]
        -t, --type <read_type>    The type of read the reader is sending [default: raw]  [possible values: raw, fsls]

## Licence

GPL3

See LICENCE.txt
