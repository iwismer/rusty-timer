use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{Point, Size},
    mono_font::{
        MonoTextStyle,
        ascii::{FONT_7X13, FONT_10X20},
    },
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Circle, Line, PrimitiveStyle, Rectangle},
    text::Text,
};

use crate::{
    layout::{DISPLAY_HEIGHT, DISPLAY_WIDTH, DIVIDER_X, compute_layout, filter_readers},
    state::{DisplayState, ReaderConnectionState, ReaderDisplayState},
};

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

const SMALL_CHAR_W: u32 = 7;
const SMALL_CHAR_H: u32 = 13;
const LARGE_CHAR_H: u32 = 20;
const INDICATOR_SIZE: u32 = 8;
const INDICATOR_GAP: u32 = 4;
const LEFT_TEXT_X: i32 = (INDICATOR_SIZE + INDICATOR_GAP) as i32;
const RIGHT_X: i32 = DIVIDER_X as i32 + 5;
/// Center X of the right column.
const RIGHT_CENTER_X: i32 = i32::midpoint(DIVIDER_X as i32, DISPLAY_WIDTH as i32);

// ---------------------------------------------------------------------------
// Public render function
// ---------------------------------------------------------------------------

/// Draw the complete display state onto `target`.
pub fn render_display<D>(target: &mut D, state: &DisplayState) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    // 1. Clear to white (Off = white on e-ink).
    target.clear(BinaryColor::Off)?;

    // 2. Vertical divider.
    let divider_x = DIVIDER_X as i32;
    Line::new(
        Point::new(divider_x, 0),
        Point::new(divider_x, DISPLAY_HEIGHT as i32 - 1),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
    .draw(target)?;

    // 3. Left column — reader blocks.
    let visible = filter_readers(&state.readers);
    let metrics = compute_layout(visible.len());
    for (reader, &y) in visible.iter().zip(metrics.reader_y_positions.iter()) {
        draw_reader(target, reader, y as i32)?;
    }

    // 4. Right column — total reads (large, centered) + label.
    let total_str = format!("{}", state.total_reads);
    let large_style = MonoTextStyle::new(&FONT_10X20, BinaryColor::On);
    let small_style = MonoTextStyle::new(&FONT_7X13, BinaryColor::On);

    // Center the large number: estimate pixel width from character count.
    let total_chars = total_str.len() as i32;
    let large_char_w = 10_i32; // FONT_10X20 is 10 wide
    let total_text_w = total_chars * large_char_w;
    let total_text_x = RIGHT_CENTER_X - total_text_w / 2;
    let total_y = LARGE_CHAR_H as i32 + 4;

    Text::new(&total_str, Point::new(total_text_x, total_y), large_style).draw(target)?;

    // "total reads" label centered below the number.
    let label = "total reads";
    let label_chars = label.len() as i32;
    let label_text_w = label_chars * SMALL_CHAR_W as i32;
    let label_x = RIGHT_CENTER_X - label_text_w / 2;
    let label_y = total_y + LARGE_CHAR_H as i32 / 2 + SMALL_CHAR_H as i32;

    Text::new(label, Point::new(label_x, label_y), small_style).draw(target)?;

    // 5. Right column — status info below the reads section.
    let section_start_y = label_y + SMALL_CHAR_H as i32 + 4;
    let mut info_y = section_start_y;

    // IP address.
    if let Some(ref ip) = state.local_ip {
        Text::new(ip, Point::new(RIGHT_X, info_y), small_style).draw(target)?;
        info_y += SMALL_CHAR_H as i32 + 2;
    }

    // Server status: indicator square + "Server".
    {
        let sq_y = info_y - INDICATOR_SIZE as i32 + 2;
        draw_filled_square(target, Point::new(RIGHT_X, sq_y), state.server_connected)?;
        let text_x = RIGHT_X + INDICATOR_SIZE as i32 + INDICATOR_GAP as i32;
        Text::new("Server", Point::new(text_x, info_y), small_style).draw(target)?;
        info_y += SMALL_CHAR_H as i32 + 2;
    }

    // CPU temperature.
    if let Some(temp) = state.cpu_temp_celsius {
        let temp_str = format!("{temp:.1}C");
        Text::new(&temp_str, Point::new(RIGHT_X, info_y), small_style).draw(target)?;
        info_y += SMALL_CHAR_H as i32 + 2;
    }

    // Battery (if present).
    if let Some(bat) = state.battery {
        let charging_char = if bat.charging { "+" } else { "" };
        let bat_str = format!("Bat:{}{}%", charging_char, bat.percent);
        Text::new(&bat_str, Point::new(RIGHT_X, info_y), small_style).draw(target)?;
        info_y += SMALL_CHAR_H as i32 + 2;
    }
    let _ = info_y; // suppress unused warning when battery is last

    // Forwarder name at the bottom of the right column.
    if let Some(ref name) = state.forwarder_name {
        let name_y = DISPLAY_HEIGHT as i32 - 3;
        Text::new(name, Point::new(RIGHT_X, name_y), small_style).draw(target)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helper: draw one reader block (two lines)
// ---------------------------------------------------------------------------

fn draw_reader<D>(target: &mut D, reader: &ReaderDisplayState, y: i32) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    let small_style = MonoTextStyle::new(&FONT_7X13, BinaryColor::On);

    // Connection indicator, vertically centered across both lines.
    // Each reader block is 2 lines (SMALL_CHAR_H) + 2px gap = 28px total.
    let block_h = SMALL_CHAR_H as i32 * 2 + 2;
    let indicator_y = y + (block_h - INDICATOR_SIZE as i32) / 2;
    draw_connection_indicator(target, Point::new(0, indicator_y), reader.state)?;

    let ip_y = y + SMALL_CHAR_H as i32 - 2; // baseline align with indicator
    Text::new(&reader.ip, Point::new(LEFT_TEXT_X, ip_y), small_style).draw(target)?;

    // Line 2: drift + session reads.
    let line2_y = y + SMALL_CHAR_H as i32 + 2;
    let drift_str = format_drift(reader.drift_ms);
    let reads_str = format!("{}r", reader.session_reads);
    let info_str = format!("{drift_str} {reads_str}");
    Text::new(
        &info_str,
        Point::new(LEFT_TEXT_X, line2_y + SMALL_CHAR_H as i32 - 2),
        small_style,
    )
    .draw(target)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Helper: format drift value
// ---------------------------------------------------------------------------

pub(crate) fn format_drift(drift_ms: Option<i64>) -> String {
    match drift_ms {
        None => "--".to_string(),
        Some(ms) => {
            if ms >= 1000 {
                ">1s".to_string()
            } else if ms <= -1000 {
                "<-1s".to_string()
            } else {
                // Clamp to ±999 ms (already guaranteed by the branches above).
                let clamped = ms.clamp(-999, 999);
                format!("{clamped}ms")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: draw connection-state indicator
// ---------------------------------------------------------------------------

fn draw_connection_indicator<D>(
    target: &mut D,
    top_left: Point,
    state: ReaderConnectionState,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    match state {
        ReaderConnectionState::Connected => {
            // Filled square.
            Rectangle::new(top_left, Size::new(INDICATOR_SIZE, INDICATOR_SIZE))
                .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
                .draw(target)
        }
        ReaderConnectionState::Connecting => {
            // Empty (stroke-only) square.
            Rectangle::new(top_left, Size::new(INDICATOR_SIZE, INDICATOR_SIZE))
                .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
                .draw(target)
        }
        ReaderConnectionState::Disconnected => {
            // Empty circle.
            Circle::new(top_left, INDICATOR_SIZE)
                .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
                .draw(target)
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: draw a boolean filled/empty square (e.g., server connected)
// ---------------------------------------------------------------------------

fn draw_filled_square<D>(target: &mut D, top_left: Point, filled: bool) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    if filled {
        Rectangle::new(top_left, Size::new(INDICATOR_SIZE, INDICATOR_SIZE))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(target)
    } else {
        Rectangle::new(top_left, Size::new(INDICATOR_SIZE, INDICATOR_SIZE))
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(target)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::prelude::OriginDimensions;

    // A no-op DrawTarget large enough for the full 250×122 display.
    struct NoopDisplay;

    impl DrawTarget for NoopDisplay {
        type Color = BinaryColor;
        type Error = core::convert::Infallible;

        fn draw_iter<I>(&mut self, _pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            Ok(())
        }
    }

    impl OriginDimensions for NoopDisplay {
        fn size(&self) -> Size {
            Size::new(250, 122)
        }
    }

    fn make_reader(
        ip: &str,
        state: ReaderConnectionState,
        drift_ms: Option<i64>,
        session_reads: u64,
    ) -> crate::state::ReaderDisplayState {
        crate::state::ReaderDisplayState {
            ip: ip.to_string(),
            state,
            drift_ms,
            session_reads,
        }
    }

    #[test]
    fn render_initial_state_does_not_panic() {
        let state = DisplayState::initial();
        let mut display = NoopDisplay;
        render_display(&mut display, &state).unwrap();
    }

    #[test]
    fn render_populated_state_does_not_panic() {
        let state = DisplayState {
            forwarder_name: Some("fwd-01".to_string()),
            local_ip: Some("192.168.1.100".to_string()),
            server_connected: true,
            readers: vec![
                make_reader(
                    "192.168.1.10",
                    ReaderConnectionState::Connected,
                    Some(12),
                    42,
                ),
                make_reader("192.168.1.11", ReaderConnectionState::Disconnected, None, 0),
            ],
            total_reads: 1234,
            cpu_temp_celsius: Some(52.3),
            battery: Some(crate::state::BatteryState {
                percent: 87,
                charging: false,
            }),
        };
        let mut display = NoopDisplay;
        render_display(&mut display, &state).unwrap();
    }

    #[test]
    fn render_zero_readers_does_not_panic() {
        let state = DisplayState {
            readers: vec![],
            ..DisplayState::initial()
        };
        let mut display = NoopDisplay;
        render_display(&mut display, &state).unwrap();
    }

    #[test]
    fn render_four_readers_does_not_panic() {
        let state = DisplayState {
            readers: vec![
                make_reader("10.0.0.1", ReaderConnectionState::Connected, Some(5), 10),
                make_reader("10.0.0.2", ReaderConnectionState::Connecting, Some(-340), 0),
                make_reader("10.0.0.3", ReaderConnectionState::Disconnected, None, 0),
                make_reader("10.0.0.4", ReaderConnectionState::Connected, Some(999), 88),
            ],
            total_reads: 98,
            ..DisplayState::initial()
        };
        let mut display = NoopDisplay;
        render_display(&mut display, &state).unwrap();
    }

    #[test]
    fn render_five_readers_does_not_panic() {
        let state = DisplayState {
            readers: vec![
                make_reader("10.0.0.1", ReaderConnectionState::Connected, Some(5), 10),
                make_reader("10.0.0.2", ReaderConnectionState::Connected, Some(10), 5),
                make_reader("10.0.0.3", ReaderConnectionState::Connected, Some(15), 3),
                make_reader("10.0.0.4", ReaderConnectionState::Connected, Some(20), 1),
                make_reader("10.0.0.5", ReaderConnectionState::Disconnected, None, 0),
            ],
            total_reads: 19,
            ..DisplayState::initial()
        };
        let mut display = NoopDisplay;
        render_display(&mut display, &state).unwrap();
    }

    #[test]
    fn render_no_battery_does_not_panic() {
        let state = DisplayState {
            forwarder_name: Some("edge-node".to_string()),
            local_ip: Some("10.0.0.50".to_string()),
            server_connected: false,
            readers: vec![make_reader(
                "10.0.0.1",
                ReaderConnectionState::Connected,
                Some(0),
                7,
            )],
            total_reads: 7,
            cpu_temp_celsius: Some(44.0),
            battery: None,
        };
        let mut display = NoopDisplay;
        render_display(&mut display, &state).unwrap();
    }

    #[test]
    fn render_no_ip_no_name_does_not_panic() {
        let state = DisplayState {
            forwarder_name: None,
            local_ip: None,
            server_connected: false,
            readers: vec![],
            total_reads: 0,
            cpu_temp_celsius: None,
            battery: None,
        };
        let mut display = NoopDisplay;
        render_display(&mut display, &state).unwrap();
    }

    #[test]
    fn format_drift_values() {
        assert_eq!(format_drift(None), "--");
        assert_eq!(format_drift(Some(0)), "0ms");
        assert_eq!(format_drift(Some(12)), "12ms");
        assert_eq!(format_drift(Some(-340)), "-340ms");
        assert_eq!(format_drift(Some(999)), "999ms");
        assert_eq!(format_drift(Some(1000)), ">1s");
        assert_eq!(format_drift(Some(-1000)), "<-1s");
        assert_eq!(format_drift(Some(5000)), ">1s");
    }
}
