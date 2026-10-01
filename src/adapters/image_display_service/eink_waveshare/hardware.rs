//! Raspberry Pi wiring for the Waveshare e-Paper HAT (BCM pin numbers).

use anyhow::Context;
use linux_embedded_hal::gpio_cdev::{Chip, LineRequestFlags};
use linux_embedded_hal::spidev::{SpiModeFlags, SpidevOptions};
use linux_embedded_hal::{CdevPin, Delay, SpidevDevice};

use super::eink_driver::EPD7in5V2;

const GPIO_CHIP: &str = "/dev/gpiochip0";
const SPI_DEVICE: &str = "/dev/spidev0.0";
const SPI_SPEED_HZ: u32 = 4_000_000;
const RST_PIN: u32 = 17;
const DC_PIN: u32 = 25;
const BUSY_PIN: u32 = 24;
const PWR_PIN: u32 = 18;

pub type Panel = EPD7in5V2<SpidevDevice, CdevPin, CdevPin, CdevPin, Delay>;

/// Powers the HAT while held; cuts power on drop. Chip select is driven by
/// the kernel's spidev (CE0).
pub struct PoweredPanel {
    pub panel: Panel,
    power: CdevPin,
}

impl PoweredPanel {
    pub fn open() -> anyhow::Result<Self> {
        let mut chip = Chip::new(GPIO_CHIP).with_context(|| format!("Failed to open {GPIO_CHIP}"))?;
        let mut pin = |offset: u32, flags: LineRequestFlags, initial: u8, label: &str| {
            let handle = chip
                .get_line(offset)
                .and_then(|line| line.request(flags, initial, label))
                .with_context(|| format!("Failed to request GPIO {offset} ({label})"))?;
            CdevPin::new(handle).with_context(|| format!("Failed to set up GPIO {offset}"))
        };

        let power = pin(PWR_PIN, LineRequestFlags::OUTPUT, 1, "eink-pwr")?;
        let rst = pin(RST_PIN, LineRequestFlags::OUTPUT, 1, "eink-rst")?;
        let dc = pin(DC_PIN, LineRequestFlags::OUTPUT, 0, "eink-dc")?;
        let busy = pin(BUSY_PIN, LineRequestFlags::INPUT, 0, "eink-busy")?;

        let mut spi = SpidevDevice::open(SPI_DEVICE)
            .with_context(|| format!("Failed to open {SPI_DEVICE} (is SPI enabled?)"))?;
        spi.configure(
            &SpidevOptions::new()
                .max_speed_hz(SPI_SPEED_HZ)
                .mode(SpiModeFlags::SPI_MODE_0)
                .build(),
        )
        .context("Failed to configure SPI")?;

        Ok(PoweredPanel {
            panel: EPD7in5V2::new(spi, dc, rst, busy, Delay),
            power,
        })
    }
}

impl Drop for PoweredPanel {
    fn drop(&mut self) {
        use embedded_hal::digital::OutputPin;
        let _ = self.power.set_low();
    }
}
