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

The SvelteKit build output is packaged by Tauri via `src-tauri/tauri.conf.json` (`frontendDist = "../build"`) when building the Windows desktop app.

## Receiver mode UX (v1.2)

- The UI supports `live`, `race`, and `targeted_replay` mode payloads.
- Earliest epoch overrides are available for live mode.
- Targeted replay supports explicit per-row selections.
