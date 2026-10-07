#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

//! Driver for the Waveshare 7.5" V2 (800x480, black/white) e-paper panel.
//!
//! Port of the Python `epd7in5_V2.py` used by eink-train-display, including its
//! custom init sequence and look-up tables, over the `embedded-hal` traits.

use embedded_hal::delay::DelayNs;
use embedded_hal::digital::{InputPin, OutputPin};
use embedded_hal::spi::SpiDevice;

pub const WIDTH: u32 = 800;
pub const HEIGHT: u32 = 480;
/// Bytes in a full frame: one bit per pixel, rows packed MSB first.
pub const BUFFER_SIZE: usize = (WIDTH as usize / 8) * HEIGHT as usize;

/// Linux spidev rejects single transfers larger than its buffer (4096 by default).
const SPI_CHUNK: usize = 4096;

const VOLTAGE_FRAME: [u8; 7] = [0x6, 0x3F, 0x3F, 0x11, 0x24, 0x7, 0x17];

#[rustfmt::skip]
const LUT_VCOM: [u8; 42] = [
    0x0, 0xF, 0xF, 0x0, 0x0, 0x1,
    0x0, 0xF, 0x1, 0xF, 0x1, 0x2,
    0x0, 0xF, 0xF, 0x0, 0x0, 0x1,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
];

#[rustfmt::skip]
const LUT_WW: [u8; 42] = [
    0x10, 0xF, 0xF, 0x0, 0x0, 0x1,
    0x84, 0xF, 0x1, 0xF, 0x1, 0x2,
    0x20, 0xF, 0xF, 0x0, 0x0, 0x1,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
];

// The Python source uses identical tables for BW and WW.
const LUT_BW: [u8; 42] = LUT_WW;

#[rustfmt::skip]
const LUT_WB: [u8; 42] = [
    0x80, 0xF, 0xF, 0x0, 0x0, 0x1,
    0x84, 0xF, 0x1, 0xF, 0x1, 0x2,
    0x40, 0xF, 0xF, 0x0, 0x0, 0x1,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
];

// ...and for WB and BB.
const LUT_BB: [u8; 42] = LUT_WB;

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
    BUSY: InputPin,
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
        self.reset()?;

        self.send_command(0x01)?; // POWER SETTING
        self.send_data(&[
            0x17,             // internal power
            VOLTAGE_FRAME[6], // VGH & VGL
            VOLTAGE_FRAME[1], // VSH
            VOLTAGE_FRAME[2], // VSL
            VOLTAGE_FRAME[3], // VSHR
        ])?;

        self.send_command(0x82)?; // VCOM DC SETTING
        self.send_data(&[VOLTAGE_FRAME[4]])?;

        self.send_command(0x06)?; // BOOSTER SETTING
        self.send_data(&[0x27, 0x27, 0x2F, 0x17])?;

        self.send_command(0x30)?; // OSC SETTING (3C=50Hz, 3A=100Hz)
        self.send_data(&[VOLTAGE_FRAME[0]])?;

        self.send_command(0x04)?; // POWER ON
        self.delay.delay_ms(100);
        self.wait_until_idle()?;

        self.send_command(0x00)?; // PANEL SETTING
        self.send_data(&[0x3F])?;

        self.send_command(0x61)?; // RESOLUTION SETTING: 800 source, 480 gate
        self.send_data(&[0x03, 0x20, 0x01, 0xE0])?;

        self.send_command(0x15)?;
        self.send_data(&[0x00])?;

        self.send_command(0x50)?; // VCOM AND DATA INTERVAL SETTING
        self.send_data(&[0x10, 0x07])?;

        self.send_command(0x60)?; // TCON SETTING
        self.send_data(&[0x22])?;

        self.send_command(0x65)?; // RESOLUTION SETTING (gate start)
        self.send_data(&[0x00, 0x00, 0x00, 0x00])?;

        self.set_lut()
    }

    /// Show a full frame: 1 bit per pixel, MSB first, where 1 is black and 0 is white.
    pub fn display(&mut self, frame: &[u8]) -> Result<(), Error<SPI::Error>> {
        if frame.len() != BUFFER_SIZE {
            return Err(Error::BadBufferLength {
                expected: BUFFER_SIZE,
                actual: frame.len(),
            });
        }
        self.send_command(0x13)?;
        self.send_data(frame)?;
        self.refresh()
    }

    /// Fill the panel with white.
    pub fn clear(&mut self) -> Result<(), Error<SPI::Error>> {
        let white = [0x00u8; BUFFER_SIZE];
        self.send_command(0x10)?;
        self.send_data(&white)?;
        self.send_command(0x13)?;
        self.send_data(&white)?;
        self.refresh()
    }

    /// Power the panel off and enter deep sleep; `init` must be called to wake it.
    pub fn sleep(&mut self) -> Result<(), Error<SPI::Error>> {
        self.send_command(0x02)?; // POWER OFF
        self.wait_until_idle()?;
        self.send_command(0x07)?; // DEEP SLEEP
        self.send_data(&[0xA5])?;
        self.delay.delay_ms(2000);
        Ok(())
    }

    fn refresh(&mut self) -> Result<(), Error<SPI::Error>> {
        self.send_command(0x12)?; // DISPLAY REFRESH
        self.delay.delay_ms(100);
        self.wait_until_idle()
    }

    fn set_lut(&mut self) -> Result<(), Error<SPI::Error>> {
        for (command, lut) in [
            (0x20, &LUT_VCOM),
            (0x21, &LUT_WW),
            (0x22, &LUT_BW),
            (0x23, &LUT_WB),
            (0x24, &LUT_BB),
        ] {
            self.send_command(command)?;
            self.send_data(lut)?;
        }
        Ok(())
    }

    fn reset(&mut self) -> Result<(), Error<SPI::Error>> {
        self.rst.set_high().map_err(|_| Error::ResetPin)?;
        self.delay.delay_ms(20);
        self.rst.set_low().map_err(|_| Error::ResetPin)?;
        self.delay.delay_ms(2);
        self.rst.set_high().map_err(|_| Error::ResetPin)?;
        self.delay.delay_ms(20);
        Ok(())
    }

    fn send_command(&mut self, command: u8) -> Result<(), Error<SPI::Error>> {
        self.dc.set_low().map_err(|_| Error::DataCommandPin)?;
        self.spi.write(&[command]).map_err(Error::Spi)
    }

    fn send_data(&mut self, data: &[u8]) -> Result<(), Error<SPI::Error>> {
        self.dc.set_high().map_err(|_| Error::DataCommandPin)?;
        for chunk in data.chunks(SPI_CHUNK) {
            self.spi.write(chunk).map_err(Error::Spi)?;
        }
        Ok(())
    }

    /// The V2 panel signals busy by holding BUSY low; it must be polled with
    /// the "get status" command (0x71) for the pin to update.
    fn wait_until_idle(&mut self) -> Result<(), Error<SPI::Error>> {
        loop {
            self.send_command(0x71)?;
            if self.busy.is_high().map_err(|_| Error::BusyPin)? {
                break;
            }
            self.delay.delay_ms(5);
        }
        self.delay.delay_ms(20);
        Ok(())
    }
}

#[derive(Debug)]
pub enum Error<SpiError> {
    Spi(SpiError),
    DataCommandPin,
    ResetPin,
    BusyPin,
    BadBufferLength { expected: usize, actual: usize },
}

impl<SpiError: std::fmt::Debug> std::fmt::Display for Error<SpiError> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Spi(e) => write!(f, "SPI error: {e:?}"),
            Error::DataCommandPin => write!(f, "failed to drive the data/command pin"),
            Error::ResetPin => write!(f, "failed to drive the reset pin"),
            Error::BusyPin => write!(f, "failed to read the busy pin"),
            Error::BadBufferLength { expected, actual } => {
                write!(f, "frame buffer is {actual} bytes, expected {expected}")
            }
        }
    }
}

impl<SpiError: std::fmt::Debug> std::error::Error for Error<SpiError> {}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::convert::Infallible;
    use std::rc::Rc;

    use embedded_hal::digital::{ErrorType as PinErrorType, InputPin, OutputPin};
    use embedded_hal::spi::{ErrorType as SpiErrorType, Operation, SpiDevice};

    use super::*;

    /// Everything written to the panel, as (is_command, bytes).
    type Log = Rc<RefCell<Vec<(bool, Vec<u8>)>>>;

    struct FakeDc(Rc<RefCell<bool>>);
    struct FakeSpi(Log, Rc<RefCell<bool>>);
    struct FakeRst;
    /// Reports busy (low) for the first `busy_reads` polls, then idle.
    struct FakeBusy(u32);
    struct NoDelay;

    impl PinErrorType for FakeDc {
        type Error = Infallible;
    }
    impl PinErrorType for FakeRst {
        type Error = Infallible;
    }
    impl PinErrorType for FakeBusy {
        type Error = Infallible;
    }
    impl SpiErrorType for FakeSpi {
        type Error = Infallible;
    }

    impl OutputPin for FakeDc {
        fn set_low(&mut self) -> Result<(), Infallible> {
            *self.0.borrow_mut() = false;
            Ok(())
        }
        fn set_high(&mut self) -> Result<(), Infallible> {
            *self.0.borrow_mut() = true;
            Ok(())
        }
    }
    impl OutputPin for FakeRst {
        fn set_low(&mut self) -> Result<(), Infallible> {
            Ok(())
        }
        fn set_high(&mut self) -> Result<(), Infallible> {
            Ok(())
        }
    }
    impl InputPin for FakeBusy {
        fn is_high(&mut self) -> Result<bool, Infallible> {
            if self.0 == 0 {
                return Ok(true);
            }
            self.0 -= 1;
            Ok(false)
        }
        fn is_low(&mut self) -> Result<bool, Infallible> {
            self.is_high().map(|h| !h)
        }
    }
    impl SpiDevice for FakeSpi {
        fn transaction(&mut self, ops: &mut [Operation<'_, u8>]) -> Result<(), Infallible> {
            for op in ops {
                if let Operation::Write(bytes) = op {
                    self.0
                        .borrow_mut()
                        .push((!*self.1.borrow(), bytes.to_vec()));
                }
            }
            Ok(())
        }
    }
    impl DelayNs for NoDelay {
        fn delay_ns(&mut self, _ns: u32) {}
    }

    fn panel(busy_reads: u32) -> (EPD7in5V2<FakeSpi, FakeDc, FakeRst, FakeBusy, NoDelay>, Log) {
        let log: Log = Rc::default();
        let dc_state: Rc<RefCell<bool>> = Rc::default();
        let panel = EPD7in5V2::new(
            FakeSpi(log.clone(), dc_state.clone()),
            FakeDc(dc_state),
            FakeRst,
            FakeBusy(busy_reads),
            NoDelay,
        );
        (panel, log)
    }

    fn commands(log: &Log) -> Vec<u8> {
        log.borrow()
            .iter()
            .filter(|(c, _)| *c)
            .map(|(_, b)| b[0])
            .collect()
    }

    #[test]
    fn display_sends_frame_in_chunks_then_refreshes_and_polls_busy() {
        let (mut panel, log) = panel(3);

        panel.display(&[0xAB; BUFFER_SIZE]).unwrap();

        // 0x13 data, refresh, then status polls until BUSY goes high (3 busy + 1 idle).
        assert_eq!(commands(&log), vec![0x13, 0x12, 0x71, 0x71, 0x71, 0x71]);
        let log = log.borrow();
        let data: Vec<_> = log.iter().filter(|(c, _)| !*c).collect();
        assert!(data.iter().all(|(_, b)| b.len() <= SPI_CHUNK));
        assert_eq!(
            data.iter().map(|(_, b)| b.len()).sum::<usize>(),
            BUFFER_SIZE
        );
    }

    #[test]
    fn display_rejects_wrong_sized_frame() {
        let (mut panel, log) = panel(0);

        let result = panel.display(&[0; 10]);

        assert!(matches!(
            result,
            Err(Error::BadBufferLength {
                expected: BUFFER_SIZE,
                actual: 10
            })
        ));
        assert!(log.borrow().is_empty());
    }

    #[test]
    fn init_loads_all_five_luts_after_panel_setup() {
        let (mut panel, log) = panel(0);

        panel.init().unwrap();

        let cmds = commands(&log);
        assert_eq!(&cmds[..3], &[0x01, 0x82, 0x06]);
        assert_eq!(&cmds[cmds.len() - 5..], &[0x20, 0x21, 0x22, 0x23, 0x24]);
        assert!(
            log.borrow()
                .iter()
                .any(|(c, b)| !*c && b == &LUT_WB.to_vec())
        );
    }

    #[test]
    fn sleep_powers_off_then_deep_sleeps() {
        let (mut panel, log) = panel(0);

        panel.sleep().unwrap();

        assert_eq!(commands(&log), vec![0x02, 0x71, 0x07]);
        assert_eq!(log.borrow().last().unwrap().1, vec![0xA5]);
    }
}
