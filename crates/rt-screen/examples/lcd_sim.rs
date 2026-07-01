//! Desktop simulator for the color 240x320 LCD display layout.
//!
//! Static mode (default): renders hardcoded sample data.
//! ```text
//! cargo run -p rt-screen --example lcd_sim --features simulator
//! ```
//!
//! Live mode: polls a running forwarder for real display state.
//! ```text
//! cargo run -p rt-screen --example lcd_sim --features simulator -- --url http://127.0.0.1:8787
//! ```
//!
//! Headless PNG dump (`--once`): renders a single frame and writes a PNG
//! without opening a window. The output path comes from `EG_SIMULATOR_DUMP_RAW`
//! (defaults to `lcd-status.png`). Add `--require-live` to force a successful
//! live fetch (exits nonzero if the fetch/decode fails).
//! ```text
//! EG_SIMULATOR_DUMP_RAW=out.png cargo run -p rt-screen --example lcd_sim --features simulator -- --once --require-live --url http://127.0.0.1:8787
//! ```

#[cfg(not(feature = "simulator"))]
fn main() {
    eprintln!("Re-run with --features simulator");
}

#[cfg(feature = "simulator")]
#[allow(clippy::too_many_lines)]
fn main() {
    use embedded_graphics::{pixelcolor::Rgb565, prelude::*};
    use embedded_graphics_simulator::{
        OutputSettingsBuilder, SimulatorDisplay, SimulatorEvent, Window,
    };
    use rt_screen::lcd::render::render_display;
    use rt_screen::state::{BatteryState, DisplayState, ReaderConnectionState, ReaderDisplayState};

    const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8787";

    // ---- Argument parsing (simple manual parse) ----
    let args: Vec<String> = std::env::args().collect();
    let base_url = args
        .iter()
        .position(|a| a == "--url")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let once = args.iter().any(|a| a == "--once");
    let require_live = args.iter().any(|a| a == "--require-live");

    // Sample data used for the static preview and non-strict `--once` fallback.
    let sample_state = || DisplayState {
        forwarder_name: Some("Start Line".to_owned()),
        local_ip: Some("192.168.0.100".to_owned()),
        p2p_connected: true,
        readers: vec![
            ReaderDisplayState {
                ip: "192.168.0.155".to_owned(),
                state: ReaderConnectionState::Connected,
                drift_ms: Some(12),
                session_reads: 842,
            },
            ReaderDisplayState {
                ip: "192.168.0.156".to_owned(),
                state: ReaderConnectionState::Connecting,
                drift_ms: Some(-45),
                session_reads: 0,
            },
            ReaderDisplayState {
                ip: "192.168.0.200".to_owned(),
                state: ReaderConnectionState::Disconnected,
                drift_ms: None,
                session_reads: 0,
            },
        ],
        total_reads: 1234,
        cpu_temp_celsius: Some(52.0),
        battery: Some(BatteryState {
            percent: 87,
            charging: true,
        }),
    };

    let output_settings = OutputSettingsBuilder::new().scale(2).build();

    let fetch_state = |url: &str| -> Result<DisplayState, reqwest::Error> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .expect("HTTP client");
        client
            .get(url)
            .send()
            .and_then(reqwest::blocking::Response::json::<DisplayState>)
    };

    if once {
        // ---- Once / headless PNG dump mode ----
        let base = base_url.as_deref().unwrap_or(DEFAULT_BASE_URL);
        let url = format!("{base}/api/v1/display-state");

        let png_path =
            std::env::var("EG_SIMULATOR_DUMP_RAW").unwrap_or_else(|_| "lcd-status.png".to_owned());

        let state = match fetch_state(&url) {
            Ok(state) => state,
            Err(e) => {
                if require_live {
                    eprintln!("live fetch/decode failed ({url}): {e}");
                    std::process::exit(1);
                }
                eprintln!("live fetch failed ({url}): {e}; falling back to sample data");
                sample_state()
            }
        };

        let mut display: SimulatorDisplay<Rgb565> = SimulatorDisplay::new(Size::new(240, 320));
        render_display(&mut display, &state).unwrap();

        let output_image = display.to_rgb_output_image(&output_settings);
        output_image.save_png(&png_path).expect("save png");
        println!("{png_path}");
        std::process::exit(0);
    }

    if let Some(base) = base_url {
        // ---- Live mode: poll the forwarder and update the display. ----
        let url = format!("{base}/api/v1/display-state");
        let mut display: SimulatorDisplay<Rgb565> = SimulatorDisplay::new(Size::new(240, 320));
        let mut window = Window::new("LCD Simulator — Live", &output_settings);

        eprintln!("Polling {url} every 1s (Ctrl-C to quit)");

        loop {
            match fetch_state(&url) {
                Ok(state) => {
                    render_display(&mut display, &state).unwrap();
                }
                Err(e) => {
                    eprintln!("fetch error: {e}");
                }
            }

            window.update(&display);
            for event in window.events() {
                if matches!(event, SimulatorEvent::Quit) {
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    } else {
        // ---- Static mode: render hardcoded sample data. ----
        let mut display: SimulatorDisplay<Rgb565> = SimulatorDisplay::new(Size::new(240, 320));
        render_display(&mut display, &sample_state()).unwrap();
        Window::new("LCD Simulator (240x320)", &output_settings).show_static(&display);
    }
}
