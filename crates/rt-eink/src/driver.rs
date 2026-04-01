//! Thin wrapper around `epd-waveshare` for the Waveshare 2.13" E-Ink HAT.
//!
//! Only compiled when the `hardware` feature is enabled.

use crate::state::DisplayModel;
use rppal::gpio::Gpio;
use rppal::spi::{Bus, Mode, SlaveSelect, Spi};
use tracing::{info, warn};

/// GPIO pin assignments for the Waveshare 2.13" E-Ink HAT.
const PIN_DC: u8 = 25;
const PIN_RST: u8 = 17;
const PIN_BUSY: u8 = 24;
const PIN_CS: u8 = 8;

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
    model: DisplayModel,
    spi: Spi,
    _gpio: Gpio,
}

impl EinkDriver {
    /// Initialize the SPI bus, GPIO pins, and e-ink display controller.
    pub fn new(model: DisplayModel) -> Result<Self, DriverError> {
        let spi = Spi::new(Bus::Spi0, SlaveSelect::Ss0, SPI_CLOCK_HZ, Mode::Mode0)
            .map_err(|e| DriverError::Spi(e.to_string()))?;

        let gpio = Gpio::new().map_err(|e| DriverError::Gpio(e.to_string()))?;

        let _dc = gpio
            .get(PIN_DC)
            .map_err(|e| DriverError::Gpio(format!("DC pin {PIN_DC}: {e}")))?;
        let _rst = gpio
            .get(PIN_RST)
            .map_err(|e| DriverError::Gpio(format!("RST pin {PIN_RST}: {e}")))?;
        let _busy = gpio
            .get(PIN_BUSY)
            .map_err(|e| DriverError::Gpio(format!("BUSY pin {PIN_BUSY}: {e}")))?;
        let _cs = gpio
            .get(PIN_CS)
            .map_err(|e| DriverError::Gpio(format!("CS pin {PIN_CS}: {e}")))?;

        info!(?model, "e-ink display driver initialized");

        Ok(Self {
            model,
            spi,
            _gpio: gpio,
        })
    }

    /// Perform a full display refresh (slow, clears ghosting).
    pub fn full_refresh(&mut self, buffer: &[u8]) -> Result<(), DriverError> {
        let _ = (buffer, self.model, &self.spi);
        warn!("full_refresh: hardware display update not yet implemented");
        Ok(())
    }

    /// Perform a partial display refresh (fast, may accumulate ghosting).
    pub fn partial_refresh(&mut self, buffer: &[u8]) -> Result<(), DriverError> {
        let _ = (buffer, self.model, &self.spi);
        warn!("partial_refresh: hardware display update not yet implemented");
        Ok(())
    }

    /// Put the display controller to sleep to reduce idle power draw.
    pub fn sleep(&mut self) -> Result<(), DriverError> {
        warn!("sleep: hardware sleep not yet implemented");
        Ok(())
    }
}
