//! Desktop simulator for the e-ink display layout.
//!
//! Static mode (default): renders hardcoded sample data.
//!   cargo run -p rt-eink --example eink_sim --features simulator
//!
//! Live mode: polls a running forwarder for real display state.
//!   cargo run -p rt-eink --example eink_sim --features simulator -- --url http://127.0.0.1:8081

#[cfg(not(feature = "simulator"))]
fn main() {
    eprintln!("Re-run with --features simulator");
}

#[cfg(feature = "simulator")]
fn main() {
    use embedded_graphics::{pixelcolor::BinaryColor, prelude::*};
    use embedded_graphics_simulator::{
        BinaryColorTheme, OutputSettingsBuilder, SimulatorDisplay, SimulatorEvent, Window,
    };
    use rt_eink::render::render_display;
    use rt_eink::state::{BatteryState, DisplayState, ReaderConnectionState, ReaderDisplayState};

    let args: Vec<String> = std::env::args().collect();
    let url = args
        .iter()
        .position(|a| a == "--url")
        .and_then(|i| args.get(i + 1))
        .map(|u| format!("{u}/api/v1/display-state"));

    let output_settings = OutputSettingsBuilder::new()
        .theme(BinaryColorTheme::Default)
        .scale(3)
        .build();

    if let Some(url) = url {
        // Live mode: poll the forwarder and update the display.
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(250, 122));
        let mut window = Window::new("E-Ink Simulator — Live", &output_settings);

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .expect("HTTP client");

        eprintln!("Polling {url} every 1s (Ctrl-C to quit)");

        loop {
            match client
                .get(&url)
                .send()
                .and_then(|r| r.json::<DisplayState>())
            {
                Ok(state) => {
                    display.clear(BinaryColor::Off).unwrap();
                    render_display(&mut display, &state).unwrap();
                }
                Err(e) => {
                    eprintln!("fetch error: {e}");
                }
            }

            window.update(&display);
            // Check for window close.
            for event in window.events() {
                if matches!(event, SimulatorEvent::Quit) {
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    } else {
        // Static mode: render hardcoded sample data.
        let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(250, 122));

        let state = DisplayState {
            forwarder_name: Some("Start Line".to_owned()),
            local_ip: Some("192.168.0.100".to_owned()),
            server_connected: true,
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

        render_display(&mut display, &state).unwrap();
        Window::new("E-Ink Simulator (250x122)", &output_settings).show_static(&display);
    }
}
