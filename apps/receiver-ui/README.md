# receiver-ui

Web UI for the rusty-timer receiver.

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

The build output is embedded in the receiver binary via `rust-embed` behind the `embed-ui` feature flag.

## Selection and replay UX (v1.1)

- The default selection mode presented to operators is `manual`.
- Default replay behavior is `resume`.
- `race/current` mode is available as an explicit opt-in path.
- Targeted replay supports explicit per-row save actions so operators choose exactly which row is persisted.
