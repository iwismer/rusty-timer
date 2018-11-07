# Rusty Timer

This contains a set of timing related utilities for the Ipico timing system.

This project requires Rust nightly to be built

## Read Streamer

This is a chip read forwarding program designed for race timing. It allows timers to overcome the connection limits on some chip reader systems.

It has been tested on the Ipico Lite reader.

Run with: ```cargo run --bin streamer [args]```
Build with ```cargo build --release --bin streamer```

## Read Emulator

This is a chip read emulation program designed for testing race timing software.
Run with: ```cargo run --bin emulator [args]```
Build with ```cargo build --release --bin emulator```

## Read Parser

This is a chip read parsing libarary designed for Ipico chip reads.
Run with: ```cargo run --bin parser [args]```
Build with ```cargo build --release --bin parser```

## Licence

See LICENCE.txt
