# server-ui

SvelteKit web UI embedded in the Rusty Timer server when the `server` crate is
built with the `embed-ui` feature. It provides the status board, admin device
approval, enrollment-token management, and SBC setup flows.

## Stack

- SvelteKit 2 + Svelte 5
- TailwindCSS
- `@sveltejs/adapter-static`
- Shared components from `@rusty-timer/shared-ui`

## Development

From the repository root:

```bash
npm install
npm run dev --workspace apps/server-ui
npm run build --workspace apps/server-ui
npm test --workspace apps/server-ui
npm run check --workspace apps/server-ui
npm run lint --workspace apps/server-ui
```

Or from this directory, run the same package scripts without `--workspace`.

## Deployment

The static build output is embedded in the server binary via `rust-embed` behind
the server crate's `embed-ui` feature flag.
