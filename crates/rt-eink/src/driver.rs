//! Thin wrapper around `epd-waveshare` for the Waveshare 2.13" E-Ink HAT.
//!
//! Only compiled when the `hardware` feature is enabled.

// The upstream crate exposes the 2.13" V3/V4 controller under the
// `epd2in13_v2` module name, selected via crate features.
use epd_waveshare::epd2in13_v2::{Display2in13, Epd2in13};
use epd_waveshare::prelude::*;
use rppal::gpio::{Gpio, InputPin, OutputPin};
use rppal::hal::Delay;
use rppal::spi::{Bus, Mode, SimpleHalSpiDevice, SlaveSelect, Spi};
use tracing::{debug, info};

/// GPIO pin assignments for the Waveshare 2.13" E-Ink HAT.
const PIN_DC: u8 = 25;
const PIN_RST: u8 = 17;
const PIN_BUSY: u8 = 24;

/// SPI clock speed in Hz (4 MHz — Waveshare recommended).
const SPI_CLOCK_HZ: u32 = 4_000_000;

/// Errors from the e-ink hardware driver.
#[derive(Debug)]
pub enum DriverError {
    Spi(String),
    Gpio(String),
    Display(String),
}

impl std::fmt::Display for DriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DriverError::Spi(e) => write!(f, "SPI error: {e}"),
            DriverError::Gpio(e) => write!(f, "GPIO error: {e}"),
            DriverError::Display(e) => write!(f, "Display error: {e}"),
        }
    }
}

impl std::error::Error for DriverError {}

/// Wraps the initialized SPI, GPIO pins, and e-ink display driver.
pub struct EinkDriver {
    spi: SimpleHalSpiDevice,
    epd: Epd2in13<SimpleHalSpiDevice, InputPin, OutputPin, OutputPin, Delay>,
    delay: Delay,
    display: Display2in13,
}

impl EinkDriver {
    /// Initialize the SPI bus, GPIO pins, and the fixed v4-targeted e-ink controller.
    pub fn new() -> Result<Self, DriverError> {
        let spi = Spi::new(Bus::Spi0, SlaveSelect::Ss0, SPI_CLOCK_HZ, Mode::Mode0)
            .map_err(|e| DriverError::Spi(e.to_string()))?;
        let mut spi = SimpleHalSpiDevice::new(spi);

        let gpio = Gpio::new().map_err(|e| DriverError::Gpio(e.to_string()))?;

        let busy = gpio
            .get(PIN_BUSY)
            .map_err(|e| DriverError::Gpio(format!("BUSY pin {PIN_BUSY}: {e}")))?
            .into_input();
        let dc = gpio
            .get(PIN_DC)
            .map_err(|e| DriverError::Gpio(format!("DC pin {PIN_DC}: {e}")))?
            .into_output();
        let rst = gpio
            .get(PIN_RST)
            .map_err(|e| DriverError::Gpio(format!("RST pin {PIN_RST}: {e}")))?
            .into_output();

        let mut delay = Delay::new();

        let epd = Epd2in13::new(&mut spi, busy, dc, rst, &mut delay, None)
            .map_err(|e| DriverError::Display(format!("EPD init: {e:?}")))?;

        let display = Display2in13::default();

        info!("e-ink display driver initialized");

        Ok(Self {
            spi,
            epd,
            delay,
            display,
        })
    }

    /// Get a mutable reference to the display framebuffer for drawing.
    pub fn display_mut(&mut self) -> &mut Display2in13 {
        &mut self.display
    }

    /// Perform a full display refresh (slow, clears ghosting).
    pub fn full_refresh(&mut self) -> Result<(), DriverError> {
        self.epd
            .set_refresh(&mut self.spi, &mut self.delay, RefreshLut::Full)
            .map_err(|e| DriverError::Display(format!("set full LUT: {e:?}")))?;
        self.epd
            .update_and_display_frame(&mut self.spi, self.display.buffer(), &mut self.delay)
            .map_err(|e| DriverError::Display(format!("full refresh: {e:?}")))?;
        debug!("full refresh complete");
        Ok(())
    }

    /// Perform a partial display refresh (fast, may accumulate ghosting).
    pub fn partial_refresh(&mut self) -> Result<(), DriverError> {
        self.epd
            .set_refresh(&mut self.spi, &mut self.delay, RefreshLut::Quick)
            .map_err(|e| DriverError::Display(format!("set partial LUT: {e:?}")))?;
        self.epd
            .update_and_display_frame(&mut self.spi, self.display.buffer(), &mut self.delay)
            .map_err(|e| DriverError::Display(format!("partial refresh: {e:?}")))?;
        debug!("partial refresh complete");
        Ok(())
    }

    /// Put the display controller to sleep to reduce idle power draw.
    pub fn sleep(&mut self) -> Result<(), DriverError> {
        self.epd
            .sleep(&mut self.spi, &mut self.delay)
            .map_err(|e| DriverError::Display(format!("sleep: {e:?}")))?;
        info!("e-ink display sleeping");
        Ok(())
    }
}
