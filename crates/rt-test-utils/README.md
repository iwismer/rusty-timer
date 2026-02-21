# rt-test-utils

Mock WebSocket infrastructure for integration testing.

## Purpose

Provides mock WebSocket server and client implementations for testing the forwarder, server, and receiver components without running real services. The mock server implements the v1 protocol handshake (hello/heartbeat) and event batch acknowledgement. Used in integration tests.

## Key types

- **`MockWsServer`** -- A lightweight WebSocket server that binds to a random port and handles the v1 protocol handshake flow. Responds to `ForwarderHello` and `ReceiverHello` with `Heartbeat` messages, acks `ForwarderEventBatch` messages, and rejects protocol violations with `ErrorMessage`. Start with `MockWsServer::start().await`.
- **`MockWsClient`** -- A WebSocket client for connecting to a `MockWsServer` (or any v1-compatible server). Provides `send_message(&WsMessage)` and `recv_message() -> WsMessage` for typed protocol communication. Connect with `MockWsClient::connect(url).await`.
