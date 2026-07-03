# rt-ui-http

Shared helpers for serving embedded SvelteKit static assets from Rust services.

Services such as `forwarder` and `server` enable their `embed-ui` feature to
compile static UI assets into the binary with `rust-embed`. Without `embed-ui`,
`rt-ui-http` returns a small fallback page that explains the UI was not embedded.

## Feature

- `embed-ui` — enables `rust-embed`/MIME support for serving compiled frontend
  assets.
