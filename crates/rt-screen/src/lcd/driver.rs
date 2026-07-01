//! ST7789 LCD hardware driver built on `mipidsi` + `rppal`.
//!
//! Only compiled on Linux with the `lcd-hardware` feature.
//!
//! Backlight policy: the backlight is held OFF until the first successful render
//! (enforced by the caller — see the forwarder integration) and is turned OFF on
//! shutdown/sleep and on any initialization failure. This avoids showing a blank
//! or garbage panel to the operator before the first frame is drawn.

use embedded_graphics::framebuffer::{Framebuffer, buffer_size};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::pixelcolor::raw::{LittleEndian, RawU16};
use embedded_graphics::prelude::*;
use mipidsi::interface::SpiInterface;
use mipidsi::options::{ColorInversion, Orientation, Rotation};
use mipidsi::{Builder, models::ST7789};
use rppal::gpio::{Gpio, OutputPin};
use rppal::hal::Delay;
use rppal::spi::{Bus, Mode, SimpleHalSpiDevice, SlaveSelect, Spi};
use tracing::{debug, info, warn};

use crate::state::{LcdConfig, LcdRotation};

/// Scratch buffer size (bytes) for the `mipidsi` SPI interface. Used to batch
/// pixel data transfers; larger values reduce SPI transaction overhead.
const SPI_BUFFER_LEN: usize = 512;

/// ST7789 panel framebuffer dimensions (portrait, native orientation).
const PANEL_WIDTH: u16 = 240;
const PANEL_HEIGHT: u16 = 320;

/// In-RAM framebuffer dimensions (must match the portrait panel; landscape
/// rotations are rejected in config validation).
const FB_WIDTH: usize = 240;
const FB_HEIGHT: usize = 320;

/// Off-screen `Rgb565` framebuffer. The full frame is composed in RAM and blitted
/// to the panel in a single pass (see [`LcdDriver::flush`]) so the operator never
/// sees the intermediate clear-to-black + incremental redraw (flicker/tearing).
type PanelFb = Framebuffer<
    Rgb565,
    RawU16,
    LittleEndian,
    FB_WIDTH,
    FB_HEIGHT,
    { buffer_size::<Rgb565>(FB_WIDTH, FB_HEIGHT) },
>;

/// Errors from the LCD hardware driver.
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

/// `mipidsi` SPI display interface over an `rppal` SPI device and DC output pin.
///
/// The interface borrows its scratch buffer for the interface's lifetime, and the
/// [`mipidsi::Display`] carries that lifetime. Storing both the buffer and the
/// display in one struct would be self-referential, so the buffer is leaked to
/// obtain a `'static` reference (a fixed, one-time allocation for the process).
type Interface = SpiInterface<'static, SimpleHalSpiDevice, OutputPin>;

/// The concrete ST7789 display type. Generic order is `Display<DI, MODEL, RST>`.
type St7789Display = mipidsi::Display<Interface, ST7789, OutputPin>;

/// Wraps the initialized SPI interface, ST7789 display, and backlight control pin.
pub struct LcdDriver {
    display: St7789Display,
    backlight: OutputPin,
    delay: Delay,
    /// Off-screen buffer; boxed because it is ~150 KiB (240x320x2).
    framebuffer: Box<PanelFb>,
}

impl LcdDriver {
    /// Initialize the SPI bus, GPIO pins, and the ST7789 controller.
    ///
    /// The backlight is acquired first and driven LOW so nothing is shown before the
    /// panel is initialized. It stays OFF on return; the caller must call
    /// [`LcdDriver::set_backlight(true)`](Self::set_backlight) only after the first
    /// successful render. On any init failure the backlight remains OFF.
    pub fn new(config: &LcdConfig) -> Result<Self, DriverError> {
        info!(
            spi_bus = config.spi_bus,
            spi_chip_select = config.spi_chip_select,
            spi_clock_hz = config.spi_clock_hz,
            dc_pin = config.dc_pin,
            rst_pin = config.rst_pin,
            backlight_pin = config.backlight_pin,
            rotation = ?config.rotation,
            "lcd driver: starting initialization"
        );

        // Acquire the backlight pin first and keep it OFF until the first render.
        let gpio = Gpio::new().map_err(|e| {
            warn!(error = %e, "lcd driver: GPIO init failed — check /dev/gpiomem permissions");
            DriverError::Gpio(e.to_string())
        })?;
        debug!("lcd driver: GPIO controller opened");

        let mut backlight = gpio
            .get(config.backlight_pin)
            .map_err(|e| {
                warn!(pin = config.backlight_pin, error = %e, "lcd driver: failed to acquire backlight pin");
                DriverError::Gpio(format!("backlight pin {}: {e}", config.backlight_pin))
            })?
            .into_output();
        backlight.set_low();
        debug!(
            pin = config.backlight_pin,
            "lcd driver: backlight pin acquired (OFF)"
        );

        // Open the SPI bus. Config validation restricts spi_bus to 0 and chip_select
        // to 0/1, so map directly onto rppal's SPI0 slave-select lines.
        let slave_select = match config.spi_chip_select {
            0 => SlaveSelect::Ss0,
            _ => SlaveSelect::Ss1,
        };
        let spi = Spi::new(Bus::Spi0, slave_select, config.spi_clock_hz, Mode::Mode0)
            .map_err(|e| {
                warn!(error = %e, "lcd driver: SPI bus init failed — is SPI enabled? Check dtparam=spi=on in /boot/firmware/config.txt (or /boot/config.txt on older OS)");
                backlight.set_low();
                DriverError::Spi(e.to_string())
            })?;
        let spi_dev = SimpleHalSpiDevice::new(spi);
        debug!("lcd driver: SPI bus opened");

        // Acquire the DC (data/command) and RST (reset) output pins.
        let dc = gpio
            .get(config.dc_pin)
            .map_err(|e| {
                warn!(pin = config.dc_pin, error = %e, "lcd driver: failed to acquire DC pin");
                backlight.set_low();
                DriverError::Gpio(format!("DC pin {}: {e}", config.dc_pin))
            })?
            .into_output();
        let rst = gpio
            .get(config.rst_pin)
            .map_err(|e| {
                warn!(pin = config.rst_pin, error = %e, "lcd driver: failed to acquire RST pin");
                backlight.set_low();
                DriverError::Gpio(format!("RST pin {}: {e}", config.rst_pin))
            })?
            .into_output();
        debug!(
            dc_pin = config.dc_pin,
            rst_pin = config.rst_pin,
            "lcd driver: GPIO pins acquired"
        );

        // Leak a fixed scratch buffer to obtain a 'static reference for the interface.
        // See the `Interface` type alias for why this is necessary.
        let buffer: &'static mut [u8] = Box::leak(Box::new([0u8; SPI_BUFFER_LEN]));
        let di = SpiInterface::new(spi_dev, dc, buffer);

        let mut delay = Delay::new();

        info!("lcd driver: sending ST7789 init sequence (this talks to the display over SPI)");
        let display = Builder::new(ST7789, di)
            .reset_pin(rst)
            .display_size(PANEL_WIDTH, PANEL_HEIGHT)
            .orientation(Orientation::new().rotate(map_rotation(config.rotation)))
            .invert_colors(ColorInversion::Inverted)
            .init(&mut delay)
            .map_err(|e| {
                warn!(error = ?e, "lcd driver: ST7789 controller init failed — check wiring and panel connection");
                // Init failed: keep the backlight OFF so we never show a blank panel.
                backlight.set_low();
                DriverError::Display(format!("init: {e:?}"))
            })?;

        info!("lcd display driver initialized successfully (backlight OFF until first render)");

        Ok(Self {
            display,
            backlight,
            delay,
            framebuffer: Box::new(PanelFb::new()),
        })
    }

    /// Get a mutable reference to the off-screen framebuffer for drawing.
    ///
    /// The renderer (`crate::lcd::render`) draws the full frame here; call
    /// [`LcdDriver::flush`] afterward to present it to the panel in one pass.
    /// The returned type implements `DrawTarget<Color = Rgb565>`.
    pub fn framebuffer_mut(&mut self) -> &mut PanelFb {
        &mut self.framebuffer
    }

    /// Blit the entire off-screen framebuffer to the panel in a single
    /// contiguous write (mipidsi sets the address window once and streams the
    /// pixels), avoiding the clear-to-black flicker of immediate-mode drawing.
    pub fn flush(&mut self) -> Result<(), DriverError> {
        self.framebuffer
            .as_image()
            .draw(&mut self.display)
            .map_err(|e| DriverError::Display(format!("flush: {e:?}")))
    }

    /// Turn the backlight on or off.
    ///
    /// The caller enables the backlight only after the first successful render.
    pub fn set_backlight(&mut self, on: bool) {
        if on {
            self.backlight.set_high();
        } else {
            self.backlight.set_low();
        }
    }

    /// Put the panel to sleep: turn the backlight OFF, then issue the controller
    /// sleep command to reduce idle power draw.
    pub fn sleep(&mut self) -> Result<(), DriverError> {
        self.backlight.set_low();
        self.display.sleep(&mut self.delay).map_err(|e| {
            warn!(error = ?e, "lcd driver: failed to put panel to sleep");
            DriverError::Display(format!("sleep: {e:?}"))
        })?;
        debug!("lcd display sleeping (backlight OFF, controller in sleep mode)");
        Ok(())
    }
}

/// Assert the display draw target uses `Rgb565`, matching the shared renderer.
const _: fn() = || {
    fn assert_rgb565<T: DrawTarget<Color = Rgb565>>() {}
    assert_rgb565::<St7789Display>();
};

/// Map the domain rotation onto the `mipidsi` rotation.
///
/// Only the portrait variants keep the 240x320 orientation the framebuffer and
/// renderer assume; the landscape variants (Deg90/Deg270) would swap the axes to
/// 320x240 and are rejected by `validate_lcd_config` before reaching the driver.
/// They are mapped here only for exhaustiveness.
fn map_rotation(rotation: LcdRotation) -> Rotation {
    match rotation {
        LcdRotation::Portrait => Rotation::Deg0,
        LcdRotation::Landscape => Rotation::Deg90,
        LcdRotation::PortraitInverted => Rotation::Deg180,
        LcdRotation::LandscapeInverted => Rotation::Deg270,
    }
}
