use embedded_hal::delay::DelayNs;
use embedded_hal::digital::OutputPin;
use embedded_hal::spi::SpiDevice;

pub struct EPD7in5V2<SPI, DC, RST, BUSY, DELAY> {
    spi: SPI,
    dc: DC,
    rst: RST,
    busy: BUSY,
    delay: DELAY,
}

impl<SPI, DC, RST, BUSY, DELAY> EPD7in5V2<SPI, DC, RST, BUSY, DELAY>
where
    SPI: SpiDevice,
    DC: OutputPin,
    RST: OutputPin,
    BUSY: embedded_hal::digital::InputPin,
    DELAY: DelayNs,
{
    pub fn new(spi: SPI, dc: DC, rst: RST, busy: BUSY, delay: DELAY) -> Self {
        EPD7in5V2 {
            spi,
            dc,
            rst,
            busy,
            delay,
        }
    }

    pub fn init(&mut self) -> Result<(), Error<SPI::Error>> {
        self.reset();

        self.send_command(0x01)?; // POWER SETTING
        self.send_data(&[0x07, 0x07, 0x3f, 0x3f])?;
        self.send_command(0x06)?; // BOOSTER SOFT START
        self.send_data(&[0x17, 0x17, 0x28, 0x17])?;

        self.send_command(0x04)?; // POWER ON
        self.wait_until_idle();

        self.send_command(0x00)?; // PANEL SETTING
        self.send_data(&[0x1f])?;

        self.send_command(0x61)?; // RESOLUTION SETTING
        self.send_data(&[0x03, 0x20, 0x01, 0xe0])?;

        self.send_command(0x15)?;
        self.send_data(&[0x00])?;

        self.send_command(0x50)?; // VCOM AND DATA INTERVAL SETTING
        self.send_data(&[0x10, 0x07])?;

        self.send_command(0x60)?; // TCON SETTING
        self.send_data(&[0x22])?;

        Ok(())
    }

    pub fn display(&mut self, image: &[u8]) -> Result<(), Error<SPI::Error>> {
        self.send_command(0x10)?;
        self.send_data(image)?;

        self.send_command(0x13)?;
        self.send_data(&image.iter().map(|&x| !x).collect::<Vec<_>>())?;

        self.send_command(0x12)?; // DISPLAY REFRESH
        self.delay.delay_ms(100);
        self.wait_until_idle();

        Ok(())
    }

    pub fn sleep(&mut self) -> Result<(), Error<SPI::Error>> {
        self.send_command(0x02)?; // POWER OFF
        self.wait_until_idle();
        self.send_command(0x07)?; // DEEP SLEEP
        self.send_data(&[0xa5])?;
        Ok(())
    }

    fn reset(&mut self) {
        self.rst.set_high().ok();
        self.delay.delay_ms(20);
        self.rst.set_low().ok();
        self.delay.delay_ms(2);
        self.rst.set_high().ok();
        self.delay.delay_ms(20);
    }

    fn send_command(&mut self, command: u8) -> Result<(), Error<SPI::Error>> {
        self.dc.set_low().map_err(|_| Error::DCPinError)?;
        self.spi.write(&[command]).map_err(Error::Spi)?;
        Ok(())
    }

    fn send_data(&mut self, data: &[u8]) -> Result<(), Error<SPI::Error>> {
        self.dc.set_high().map_err(|_| Error::DCPinError)?;
        self.spi.write(data).map_err(Error::Spi)?;
        Ok(())
    }

    fn wait_until_idle(&mut self) {
        while self.busy.is_high().unwrap_or(true) {
            self.delay.delay_ms(100);
        }
    }
}

#[derive(Debug)]
pub enum Error<SpiError> {
    Spi(SpiError),
    DCPinError,
    RSTError,
    BUSYError,
}
