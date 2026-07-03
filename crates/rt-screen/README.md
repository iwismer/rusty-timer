# rt-screen

Shared screen state and rendering support for forwarder-attached displays.

The crate contains configuration/state types for e-ink and LCD/simulator
backends plus helpers used by the forwarder when display features are enabled.
The forwarder can map legacy `[eink]` TOML config into the newer `[screen]`
shape when built with display support.

## Features

- `eink` — enables the e-ink backend support.
- `lcd` — enables LCD backend support.
- `simulator` — enables the LCD simulator example.

## Example

```bash
cargo build -p rt-screen --example lcd_sim --features simulator
```
