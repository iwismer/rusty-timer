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

## Reader controls

- The UI supports setting and clearing a reader's current epoch name through the
  forwarder's local `PUT /api/v1/streams/{reader_ip}/current-epoch/name` API.
- The UI supports advancing/resetting a stream epoch through the forwarder's
  local `POST /api/v1/streams/{reader_ip}/reset-epoch` API.
- Reader detail controls also cover clock sync, read mode, TTO state, record
  download, and record clearing through local forwarder APIs.
