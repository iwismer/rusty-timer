use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{Point, Size},
    mono_font::{
        MonoTextStyle,
        ascii::{FONT_7X13, FONT_9X15, FONT_10X20},
    },
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle},
    text::Text,
};

use crate::{
    lcd::layout::{
        DISPLAY_HEIGHT, DISPLAY_WIDTH, FOOTER_HEIGHT, HEADER_HEIGHT, MAX_VISIBLE_READERS,
        READER_ROW_HEIGHT, filter_readers, overflow_count,
    },
    state::{DisplayState, ReaderConnectionState, ReaderDisplayState},
};

// ---------------------------------------------------------------------------
// Colors
// ---------------------------------------------------------------------------

/// Near-black background.
const COLOR_BG: Rgb565 = Rgb565::new(2, 4, 4);
/// Light text.
const COLOR_TEXT: Rgb565 = Rgb565::new(28, 57, 28);
/// Dimmer/secondary text (labels, placeholders).
const COLOR_TEXT_DIM: Rgb565 = Rgb565::new(16, 34, 16);
/// Accent divider (blue).
const COLOR_ACCENT: Rgb565 = Rgb565::new(6, 24, 31);
/// Connected / healthy (green).
const COLOR_CONNECTED: Rgb565 = Rgb565::new(6, 50, 8);
/// Connecting / warning (amber).
const COLOR_CONNECTING: Rgb565 = Rgb565::new(31, 45, 0);
/// Disconnected / critical (red).
const COLOR_DISCONNECTED: Rgb565 = Rgb565::new(24, 6, 6);

// ---------------------------------------------------------------------------
// Font metrics
// ---------------------------------------------------------------------------

const SMALL_CHAR_W: u32 = 7; // FONT_7X13
const MED_CHAR_W: u32 = 9; // FONT_9X15
const LARGE_CHAR_W: u32 = 10; // FONT_10X20

const INDICATOR_SIZE: u32 = 14;

// ---------------------------------------------------------------------------
// Region geometry (240x320 portrait)
// ---------------------------------------------------------------------------

/// Baseline y of the single-line total-reads count.
const COUNT_Y: i32 = 54;
/// Top y of the reader list.
const READER_LIST_TOP: u32 = 64;
/// Top y of the footer status block.
const FOOTER_TOP: u32 = DISPLAY_HEIGHT - FOOTER_HEIGHT;
/// X of the left-column value (after its label).
const STAT_VALUE_X: i32 = 40;
/// X of the right-column label in the footer bottom row (CPU | BAT).
const STAT_COL2_X: i32 = 124;
/// X of the right-column value.
const STAT_COL2_VALUE_X: i32 = 160;

// ---------------------------------------------------------------------------
// Public render functions
// ---------------------------------------------------------------------------

/// Draw the complete display state onto `target` in `Rgb565`.
///
/// # Errors
/// Returns any error produced by the underlying [`DrawTarget`].
pub fn render_display<D>(target: &mut D, state: &DisplayState) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    target.clear(COLOR_BG)?;

    draw_header(target, state)?;
    draw_count(target, state)?;
    draw_reader_list(target, state)?;
    draw_footer(target, state)?;

    Ok(())
}

/// Draw a centered "Powered Off" message on a cleared display.
///
/// # Errors
/// Returns any error produced by the underlying [`DrawTarget`].
pub fn render_shutdown<D>(target: &mut D) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    target.clear(COLOR_BG)?;

    let msg = "Powered Off";
    let text_w = msg.len() as u32 * LARGE_CHAR_W;
    let x = center_x(text_w);
    let y = (DISPLAY_HEIGHT / 2) as i32;
    Text::new(
        msg,
        Point::new(x, y),
        MonoTextStyle::new(&FONT_10X20, COLOR_TEXT),
    )
    .draw(target)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Header — display name + P2P badge
// ---------------------------------------------------------------------------

fn draw_header<D>(target: &mut D, state: &DisplayState) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    // P2P badge occupies the right side of the header.
    let badge_square_x = (DISPLAY_WIDTH - 4 - INDICATOR_SIZE) as i32;
    let p2p_label = "P2P";
    let p2p_w = p2p_label.len() as u32 * SMALL_CHAR_W;
    let p2p_x = badge_square_x - 4 - p2p_w as i32;

    // Name (left), truncated to the space before the badge.
    let name = state.forwarder_name.as_deref().unwrap_or("forwarder");
    let name_max_px = u32::try_from(p2p_x - 4).unwrap_or(0);
    let name = truncate_to_width(name, LARGE_CHAR_W, name_max_px);
    Text::new(
        &name,
        Point::new(4, 23),
        MonoTextStyle::new(&FONT_10X20, COLOR_TEXT),
    )
    .draw(target)?;

    // P2P badge indicator + label.
    let badge_color = if state.p2p_connected {
        COLOR_CONNECTED
    } else {
        COLOR_DISCONNECTED
    };
    Rectangle::new(
        Point::new(badge_square_x, 9),
        Size::new(INDICATOR_SIZE, INDICATOR_SIZE),
    )
    .into_styled(PrimitiveStyle::with_fill(badge_color))
    .draw(target)?;
    Text::new(
        p2p_label,
        Point::new(p2p_x, 22),
        MonoTextStyle::new(&FONT_7X13, COLOR_TEXT),
    )
    .draw(target)?;

    draw_divider(target, (HEADER_HEIGHT - 1) as i32)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Count region — large total reads
// ---------------------------------------------------------------------------

fn draw_count<D>(target: &mut D, state: &DisplayState) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    // Single line: big number + " total reads" label, centered together.
    // For implausibly large counts that would overflow the width, show the
    // number alone (centered) and drop the label.
    let total_str = format!("{}", state.total_reads);
    let num_w = total_str.len() as u32 * LARGE_CHAR_W;
    let label = " total reads";
    let label_w = label.len() as u32 * MED_CHAR_W;
    let with_label = num_w + label_w <= DISPLAY_WIDTH;

    let start_x = center_x(if with_label { num_w + label_w } else { num_w });
    Text::new(
        &total_str,
        Point::new(start_x, COUNT_Y),
        MonoTextStyle::new(&FONT_10X20, COLOR_TEXT),
    )
    .draw(target)?;
    if with_label {
        Text::new(
            label,
            Point::new(start_x + num_w as i32, COUNT_Y),
            MonoTextStyle::new(&FONT_9X15, COLOR_TEXT_DIM),
        )
        .draw(target)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Reader list
// ---------------------------------------------------------------------------

fn draw_reader_list<D>(target: &mut D, state: &DisplayState) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let total = state.readers.len();
    let visible = filter_readers(&state.readers);

    // When there are more readers than fit, give up the last row to a
    // "+N more" summary so no row is silently dropped without a hint.
    let has_overflow = overflow_count(total) > 0;
    let rows_to_draw = if has_overflow {
        MAX_VISIBLE_READERS - 1
    } else {
        visible.len()
    };

    for (i, reader) in visible.iter().take(rows_to_draw).enumerate() {
        let row_top = READER_LIST_TOP + i as u32 * READER_ROW_HEIGHT;
        draw_reader(target, reader, row_top as i32)?;
    }

    if has_overflow {
        let hidden = total - rows_to_draw;
        let label = format!("+{hidden} more");
        let row_top = (READER_LIST_TOP + rows_to_draw as u32 * READER_ROW_HEIGHT) as i32;
        let text_x = 4 + INDICATOR_SIZE as i32 + 6;
        Text::new(
            &label,
            Point::new(text_x, row_top + 20),
            MonoTextStyle::new(&FONT_9X15, COLOR_TEXT_DIM),
        )
        .draw(target)?;
    }

    Ok(())
}

fn draw_reader<D>(target: &mut D, reader: &ReaderDisplayState, row_top: i32) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    // Connection indicator (filled square, colored by state), vertically centered.
    let color = match reader.state {
        ReaderConnectionState::Connected => COLOR_CONNECTED,
        ReaderConnectionState::Connecting => COLOR_CONNECTING,
        ReaderConnectionState::Disconnected => COLOR_DISCONNECTED,
    };
    let indicator_y = row_top + ((READER_ROW_HEIGHT - INDICATOR_SIZE) / 2) as i32;
    Rectangle::new(
        Point::new(4, indicator_y),
        Size::new(INDICATOR_SIZE, INDICATOR_SIZE),
    )
    .into_styled(PrimitiveStyle::with_fill(color))
    .draw(target)?;

    let text_x = 4 + INDICATOR_SIZE as i32 + 6;
    let text_max_px = u32::try_from(DISPLAY_WIDTH as i32 - text_x - 4).unwrap_or(0);

    // Line 1: IP address (large).
    let ip = truncate_to_width(&reader.ip, LARGE_CHAR_W, text_max_px);
    Text::new(
        &ip,
        Point::new(text_x, row_top + 16),
        MonoTextStyle::new(&FONT_10X20, COLOR_TEXT),
    )
    .draw(target)?;

    // Line 2: drift + session reads (readable, not tiny).
    let detail = format!(
        "drift {}   {} reads",
        format_drift(reader.drift_ms),
        reader.session_reads
    );
    let detail = truncate_to_width(&detail, MED_CHAR_W, text_max_px);
    Text::new(
        &detail,
        Point::new(text_x, row_top + 31),
        MonoTextStyle::new(&FONT_9X15, COLOR_TEXT_DIM),
    )
    .draw(target)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Footer — labeled IP / CPU / battery status lines
// ---------------------------------------------------------------------------

fn draw_footer<D>(target: &mut D, state: &DisplayState) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    draw_divider(target, (FOOTER_TOP - 1) as i32)?;

    let row1 = (FOOTER_TOP + 18) as i32; // IP (full width)
    let row2 = (FOOTER_TOP + 40) as i32; // CPU | BAT

    // Row 1: IP spans the full width (addresses are long).
    let ip_max_px = u32::try_from(DISPLAY_WIDTH as i32 - STAT_VALUE_X - 4).unwrap_or(0);
    let (ip_txt, ip_color) = match &state.local_ip {
        Some(ip) => (truncate_to_width(ip, MED_CHAR_W, ip_max_px), COLOR_TEXT),
        None => ("--".to_string(), COLOR_TEXT_DIM),
    };
    draw_stat(target, 4, STAT_VALUE_X, row1, "IP", &ip_txt, ip_color)?;

    // Row 2, left: CPU temperature.
    let (cpu_txt, cpu_color) = match state.cpu_temp_celsius {
        Some(t) => {
            let color = if t >= 75.0 {
                COLOR_DISCONNECTED
            } else if t >= 65.0 {
                COLOR_CONNECTING
            } else {
                COLOR_TEXT
            };
            (format!("{t:.1}C"), color)
        }
        None => ("--".to_string(), COLOR_TEXT_DIM),
    };
    draw_stat(target, 4, STAT_VALUE_X, row2, "CPU", &cpu_txt, cpu_color)?;

    // Row 2, right: battery / UPS.
    let (bat_txt, bat_color) = match state.battery {
        Some(b) => {
            let txt = format!("{}{}%", if b.charging { "+" } else { "" }, b.percent);
            let color = if b.charging {
                COLOR_CONNECTED
            } else if b.percent <= 20 {
                COLOR_DISCONNECTED
            } else if b.percent <= 50 {
                COLOR_CONNECTING
            } else {
                COLOR_TEXT
            };
            (txt, color)
        }
        None => ("no UPS".to_string(), COLOR_TEXT_DIM),
    };
    draw_stat(
        target,
        STAT_COL2_X,
        STAT_COL2_VALUE_X,
        row2,
        "BAT",
        &bat_txt,
        bat_color,
    )?;

    Ok(())
}

/// Draw a `LABEL value` stat: dim label at `label_x`, value in `value_color`
/// at `value_x`, sharing baseline `y`.
#[allow(clippy::too_many_arguments)]
fn draw_stat<D>(
    target: &mut D,
    label_x: i32,
    value_x: i32,
    y: i32,
    label: &str,
    value: &str,
    value_color: Rgb565,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    Text::new(
        label,
        Point::new(label_x, y),
        MonoTextStyle::new(&FONT_9X15, COLOR_TEXT_DIM),
    )
    .draw(target)?;
    Text::new(
        value,
        Point::new(value_x, y),
        MonoTextStyle::new(&FONT_9X15, value_color),
    )
    .draw(target)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Left x for horizontally centering text of `text_w` pixels, clamped to 0.
fn center_x(text_w: u32) -> i32 {
    let w = text_w.min(DISPLAY_WIDTH);
    ((DISPLAY_WIDTH - w) / 2) as i32
}

fn draw_divider<D>(target: &mut D, y: i32) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    Line::new(Point::new(0, y), Point::new(DISPLAY_WIDTH as i32 - 1, y))
        .into_styled(PrimitiveStyle::with_stroke(COLOR_ACCENT, 1))
        .draw(target)
}

/// Truncate `s` so it fits within `max_px` when rendered at `char_w` px/char.
fn truncate_to_width(s: &str, char_w: u32, max_px: u32) -> String {
    if char_w == 0 {
        return String::new();
    }
    let max_chars = (max_px / char_w) as usize;
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect()
    }
}

pub(crate) fn format_drift(drift_ms: Option<i64>) -> String {
    match drift_ms {
        None => "--".to_string(),
        Some(ms) => {
            if ms >= 1000 {
                ">1s".to_string()
            } else if ms <= -1000 {
                "<-1s".to_string()
            } else {
                let clamped = ms.clamp(-999, 999);
                format!("{clamped}ms")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::BatteryState;
    use embedded_graphics::prelude::OriginDimensions;

    /// A `DrawTarget` that records every drawn pixel for assertions.
    struct RecordingDisplay {
        pixels: Vec<(Point, Rgb565)>,
        size: Size,
    }

    impl RecordingDisplay {
        fn new() -> Self {
            Self {
                pixels: Vec::new(),
                size: Size::new(DISPLAY_WIDTH, DISPLAY_HEIGHT),
            }
        }

        /// Non-background pixels (ignores the full-screen clear fill).
        fn foreground(&self) -> impl Iterator<Item = &(Point, Rgb565)> {
            self.pixels.iter().filter(|(_, c)| *c != COLOR_BG)
        }

        fn contains_color(&self, color: Rgb565) -> bool {
            self.pixels.iter().any(|(_, c)| *c == color)
        }
    }

    impl DrawTarget for RecordingDisplay {
        type Color = Rgb565;
        type Error = core::convert::Infallible;

        fn draw_iter<I>(&mut self, iter: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            for Pixel(p, c) in iter {
                self.pixels.push((p, c));
            }
            Ok(())
        }
    }

    impl OriginDimensions for RecordingDisplay {
        fn size(&self) -> Size {
            self.size
        }
    }

    fn make_reader(
        ip: &str,
        state: ReaderConnectionState,
        drift_ms: Option<i64>,
        session_reads: u64,
    ) -> ReaderDisplayState {
        ReaderDisplayState {
            ip: ip.to_string(),
            state,
            drift_ms,
            session_reads,
        }
    }

    fn readers(n: usize) -> Vec<ReaderDisplayState> {
        (0..n)
            .map(|i| {
                make_reader(
                    &format!("192.168.1.{:03}", i + 1),
                    ReaderConnectionState::Connected,
                    Some(i as i64),
                    i as u64,
                )
            })
            .collect()
    }

    fn populated(reader_count: usize) -> DisplayState {
        DisplayState {
            forwarder_name: Some("fwd-01".to_string()),
            local_ip: Some("192.168.1.100".to_string()),
            p2p_connected: true,
            readers: readers(reader_count),
            total_reads: 12_345,
            cpu_temp_celsius: Some(52.3),
            battery: Some(BatteryState {
                percent: 87,
                charging: false,
            }),
        }
    }

    fn assert_in_bounds(display: &RecordingDisplay) {
        for (p, _) in &display.pixels {
            assert!(
                p.x >= 0 && p.x < DISPLAY_WIDTH as i32 && p.y >= 0 && p.y < DISPLAY_HEIGHT as i32,
                "pixel out of bounds: {p:?}"
            );
        }
    }

    #[test]
    fn render_initial_state() {
        let mut display = RecordingDisplay::new();
        render_display(&mut display, &DisplayState::initial()).unwrap();
        assert_in_bounds(&display);
    }

    #[test]
    fn render_populated_state() {
        let mut display = RecordingDisplay::new();
        render_display(&mut display, &populated(3)).unwrap();
        assert_in_bounds(&display);
        assert!(display.foreground().count() > 0);
    }

    #[test]
    fn render_max_readers() {
        let mut display = RecordingDisplay::new();
        render_display(&mut display, &populated(MAX_VISIBLE_READERS)).unwrap();
        assert_in_bounds(&display);
        // No overflow indicator when exactly at the cap.
        assert_eq!(overflow_count(MAX_VISIBLE_READERS), 0);
    }

    #[test]
    fn render_more_than_max_readers() {
        let state = populated(MAX_VISIBLE_READERS + 4);
        let mut display = RecordingDisplay::new();
        render_display(&mut display, &state).unwrap();
        assert_in_bounds(&display);

        // When overflowing, the last row is given up to a "+N more" summary, so
        // only MAX-1 reader rows carry a connection-indicator square (x in
        // [4, 4+INDICATOR_SIZE)).
        let last_row_top = READER_LIST_TOP + (MAX_VISIBLE_READERS as u32 - 1) * READER_ROW_HEIGHT;
        let indicator_rows: std::collections::BTreeSet<i32> = display
            .foreground()
            .filter(|(p, _)| {
                p.x >= 4
                    && p.x < 4 + INDICATOR_SIZE as i32
                    && p.y >= READER_LIST_TOP as i32
                    && p.y < last_row_top as i32
            })
            .map(|(p, _)| (p.y - READER_LIST_TOP as i32) / READER_ROW_HEIGHT as i32)
            .collect();
        assert_eq!(indicator_rows.len(), MAX_VISIBLE_READERS - 1);

        // overflow_count reports readers beyond the cap; the "+N more" text is
        // drawn in the final (given-up) reader row.
        assert_eq!(overflow_count(state.readers.len()), 4);
        let overflow_text = display
            .foreground()
            .any(|(p, _)| p.y >= last_row_top as i32 && p.y < FOOTER_TOP as i32);
        assert!(overflow_text, "expected '+N more' text in the final row");
    }

    #[test]
    fn render_shutdown_ok() {
        let mut display = RecordingDisplay::new();
        render_shutdown(&mut display).unwrap();
        assert_in_bounds(&display);
        assert!(display.contains_color(COLOR_TEXT));
    }

    #[test]
    fn color_mapping() {
        let state = DisplayState {
            readers: vec![
                make_reader("10.0.0.1", ReaderConnectionState::Connected, Some(1), 1),
                make_reader("10.0.0.2", ReaderConnectionState::Connecting, None, 0),
                make_reader("10.0.0.3", ReaderConnectionState::Disconnected, None, 0),
            ],
            ..DisplayState::initial()
        };
        let mut display = RecordingDisplay::new();
        render_display(&mut display, &state).unwrap();
        assert!(display.contains_color(COLOR_CONNECTED));
        assert!(display.contains_color(COLOR_CONNECTING));
        assert!(display.contains_color(COLOR_DISCONNECTED));
    }

    #[test]
    fn reader_sort_order() {
        let unsorted = vec![
            make_reader("10.0.0.9", ReaderConnectionState::Disconnected, None, 0),
            make_reader("10.0.0.1", ReaderConnectionState::Connected, Some(1), 1),
        ];
        let sorted = filter_readers(&unsorted);
        assert_eq!(sorted[0].state, ReaderConnectionState::Connected);
        assert_eq!(sorted[1].state, ReaderConnectionState::Disconnected);

        // The connected reader (row 0) is drawn above the disconnected one
        // (row 1): its green indicator has a smaller min-y than the red one.
        let mut display = RecordingDisplay::new();
        render_display(
            &mut display,
            &DisplayState {
                readers: unsorted,
                ..DisplayState::initial()
            },
        )
        .unwrap();
        // Restrict to the reader-list region so the header P2P badge (which
        // reuses these colors) does not skew the comparison.
        let min_y = |color: Rgb565| {
            display
                .foreground()
                .filter(|(p, c)| *c == color && p.y >= READER_LIST_TOP as i32)
                .map(|(p, _)| p.y)
                .min()
        };
        assert!(min_y(COLOR_CONNECTED).unwrap() < min_y(COLOR_DISCONNECTED).unwrap());
    }

    #[test]
    fn long_name_and_ip_truncation() {
        let state = DisplayState {
            forwarder_name: Some("a-really-long-forwarder-name-that-overflows".to_string()),
            local_ip: Some("192.168.100.200-with-a-very-long-suffix-that-overflows".to_string()),
            readers: vec![make_reader(
                "192.168.100.200-with-a-very-long-suffix-that-overflows",
                ReaderConnectionState::Connected,
                Some(12),
                999,
            )],
            ..DisplayState::initial()
        };
        let mut display = RecordingDisplay::new();
        render_display(&mut display, &state).unwrap();
        for (p, _) in display.foreground() {
            assert!(p.x < DISPLAY_WIDTH as i32, "pixel x out of bounds: {p:?}");
        }
    }

    #[test]
    fn large_read_counts() {
        let state = DisplayState {
            total_reads: u64::MAX,
            ..DisplayState::initial()
        };
        let mut display = RecordingDisplay::new();
        render_display(&mut display, &state).unwrap();
        assert_in_bounds(&display);
    }

    #[test]
    fn footer_cpu_battery_ip() {
        let state = populated(1);
        let mut display = RecordingDisplay::new();
        render_display(&mut display, &state).unwrap();
        let footer_pixels = display
            .foreground()
            .filter(|(p, _)| p.y >= FOOTER_TOP as i32)
            .count();
        assert!(footer_pixels > 0, "expected footer content");
    }

    #[test]
    fn footer_labels_present_even_without_data() {
        // Initial state has no IP / temp / battery, but the labeled slots (and
        // placeholders) must still render so the fields are discoverable.
        let mut display = RecordingDisplay::new();
        render_display(&mut display, &DisplayState::initial()).unwrap();
        let footer_pixels = display
            .foreground()
            .filter(|(p, _)| p.y >= FOOTER_TOP as i32)
            .count();
        assert!(
            footer_pixels > 0,
            "expected labeled footer even with no data"
        );
    }

    #[test]
    fn layout_bounds_within_240x320() {
        let mut display = RecordingDisplay::new();
        render_display(&mut display, &populated(MAX_VISIBLE_READERS + 4)).unwrap();
        assert_in_bounds(&display);
    }

    #[test]
    fn format_drift_values() {
        assert_eq!(format_drift(None), "--");
        assert_eq!(format_drift(Some(0)), "0ms");
        assert_eq!(format_drift(Some(-340)), "-340ms");
        assert_eq!(format_drift(Some(999)), "999ms");
        assert_eq!(format_drift(Some(1000)), ">1s");
        assert_eq!(format_drift(Some(-1000)), "<-1s");
    }

    #[test]
    fn truncate_to_width_respects_char_budget() {
        assert_eq!(truncate_to_width("hello", 7, 70), "hello");
        assert_eq!(truncate_to_width("hello", 7, 14), "he");
        assert_eq!(truncate_to_width("hello", 0, 100), "");
    }
}
