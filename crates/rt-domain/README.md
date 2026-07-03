# rt-domain

Transport-independent domain and control types shared by Rusty Timer services.

This crate keeps common concepts out of service-specific HTTP, Tauri, and iroh
layers so the forwarder, receiver, and tests can exchange typed status/control
payloads without depending on a particular transport implementation.
