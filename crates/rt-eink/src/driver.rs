//! Thin wrapper around `epd-waveshare` for the Waveshare 2.13" E-Ink HAT.
//!
//! Only compiled when the `hardware` feature is enabled.

// The upstream crate exposes the 2.13" V3/V4 controller under the
// `epd2in13_v2` module name, selected via crate features.
use epd_waveshare::epd2in13_v2::{Display2in13, Epd2in13};
use epd_waveshare::graphics::DisplayRotation;
use epd_waveshare::prelude::*;
use rppal::gpio::{Gpio, InputPin, OutputPin};
use rppal::hal::Delay;
use rppal::spi::{Bus, Mode, SimpleHalSpiDevice, SlaveSelect, Spi};
use tracing::{debug, info, warn};

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
        info!(
            spi_bus = "SPI0",
            spi_slave = "SS0",
            spi_clock_hz = SPI_CLOCK_HZ,
            pin_dc = PIN_DC,
            pin_rst = PIN_RST,
            pin_busy = PIN_BUSY,
            "e-ink driver: starting initialization"
        );

        let spi = Spi::new(Bus::Spi0, SlaveSelect::Ss0, SPI_CLOCK_HZ, Mode::Mode0)
            .map_err(|e| {
                warn!(error = %e, "e-ink driver: SPI bus init failed — is SPI enabled? Check dtparam=spi=on in /boot/firmware/config.txt (or /boot/config.txt on older OS)");
                DriverError::Spi(e.to_string())
            })?;
        let mut spi = SimpleHalSpiDevice::new(spi);
        debug!("e-ink driver: SPI bus opened");

        let gpio = Gpio::new().map_err(|e| {
            warn!(error = %e, "e-ink driver: GPIO init failed — check /dev/gpiomem permissions");
            DriverError::Gpio(e.to_string())
        })?;
        debug!("e-ink driver: GPIO controller opened");

        let busy = gpio
            .get(PIN_BUSY)
            .map_err(|e| {
                warn!(pin = PIN_BUSY, error = %e, "e-ink driver: failed to acquire BUSY pin");
                DriverError::Gpio(format!("BUSY pin {PIN_BUSY}: {e}"))
            })?
            .into_input();
        let dc = gpio
            .get(PIN_DC)
            .map_err(|e| {
                warn!(pin = PIN_DC, error = %e, "e-ink driver: failed to acquire DC pin");
                DriverError::Gpio(format!("DC pin {PIN_DC}: {e}"))
            })?
            .into_output();
        let rst = gpio
            .get(PIN_RST)
            .map_err(|e| {
                warn!(pin = PIN_RST, error = %e, "e-ink driver: failed to acquire RST pin");
                DriverError::Gpio(format!("RST pin {PIN_RST}: {e}"))
            })?
            .into_output();
        debug!("e-ink driver: GPIO pins acquired (BUSY={PIN_BUSY}, DC={PIN_DC}, RST={PIN_RST})");

        let mut delay = Delay::new();

        info!("e-ink driver: sending EPD init sequence (this talks to the display over SPI)");
        let epd = Epd2in13::new(&mut spi, busy, dc, rst, &mut delay, None)
            .map_err(|e| {
                warn!(error = ?e, "e-ink driver: EPD controller init failed — check wiring and HAT seating");
                DriverError::Display(format!("EPD init: {e:?}"))
            })?;

        let mut display = Display2in13::default();
        display.set_rotation(DisplayRotation::Rotate270);

        info!("e-ink display driver initialized successfully");

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
            .map_err(|e| {
                warn!(error = ?e, "e-ink driver: failed to set full refresh LUT");
                DriverError::Display(format!("set full LUT: {e:?}"))
            })?;
        self.epd
            .update_and_display_frame(&mut self.spi, self.display.buffer(), &mut self.delay)
            .map_err(|e| {
                warn!(error = ?e, "e-ink driver: full refresh frame update failed");
                DriverError::Display(format!("full refresh: {e:?}"))
            })?;
        debug!("e-ink driver: full refresh complete");
        Ok(())
    }

    /// Perform a partial display refresh (fast, may accumulate ghosting).
    pub fn partial_refresh(&mut self) -> Result<(), DriverError> {
        self.epd
            .set_refresh(&mut self.spi, &mut self.delay, RefreshLut::Quick)
            .map_err(|e| {
                warn!(error = ?e, "e-ink driver: failed to set partial refresh LUT");
                DriverError::Display(format!("set partial LUT: {e:?}"))
            })?;
        self.epd
            .update_and_display_frame(&mut self.spi, self.display.buffer(), &mut self.delay)
            .map_err(|e| {
                warn!(error = ?e, "e-ink driver: partial refresh frame update failed");
                DriverError::Display(format!("partial refresh: {e:?}"))
            })?;
        debug!("e-ink driver: partial refresh complete");
        Ok(())
    }

    /// Put the display controller to sleep to reduce idle power draw.
    pub fn sleep(&mut self) -> Result<(), DriverError> {
        self.epd
            .sleep(&mut self.spi, &mut self.delay)
            .map_err(|e| {
                warn!(error = ?e, "e-ink driver: failed to put display to sleep");
                DriverError::Display(format!("sleep: {e:?}"))
            })?;
        info!("e-ink display sleeping");
        Ok(())
    }
}
