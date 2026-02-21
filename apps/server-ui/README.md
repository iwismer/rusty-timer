# server-ui

Web dashboard for the rusty-timer server.

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

The build output goes to the `build/` directory and is served by the server via the `DASHBOARD_DIR` environment variable. In Docker, the UI is built during the Docker image build and served from `/srv/dashboard`.
