# forwarder-ui

Web UI for the rusty-timer forwarder.

## Stack

- SvelteKit 2 + Svelte 5
- TailwindCSS
- `@sveltejs/adapter-static`

## Development

```bash
npm install
npm run dev
npm run build
npm test
npm run check
npm run lint
npm run format
```

## Deployment

The build output is embedded in the forwarder binary via `rust-embed` behind the `embed-ui` feature flag.

## Epoch name controls

- The UI supports setting a current epoch name via the server-backed API path.
- The UI supports clearing the current epoch name via the same server-backed API path.
- These controls are available through server-backed race epoch APIs.
