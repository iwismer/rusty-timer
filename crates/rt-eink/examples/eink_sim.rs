//! Desktop simulator for the e-ink display layout.
//!
//! Run with: cargo run -p rt-eink --example eink_sim --features simulator

#[cfg(not(feature = "simulator"))]
fn main() {
    eprintln!("Re-run with --features simulator");
}

#[cfg(feature = "simulator")]
fn main() {
    use embedded_graphics::{pixelcolor::BinaryColor, prelude::*};
    use embedded_graphics_simulator::{
        BinaryColorTheme, OutputSettingsBuilder, SimulatorDisplay, Window,
    };
    use rt_eink::render::render_display;
    use rt_eink::state::{BatteryState, DisplayState, ReaderConnectionState, ReaderDisplayState};

    let output_settings = OutputSettingsBuilder::new()
        .theme(BinaryColorTheme::Default)
        .scale(3)
        .build();

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
