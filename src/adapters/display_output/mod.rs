pub mod eink_waveshare;
pub mod file_output;
pub use self::file_output::output::FileOutputAdapter;

pub use self::eink_waveshare::output::EinkWaveshareAdapter;

pub enum DisplayOutputAdapter {
    File(FileOutputAdapter),
    EinkWaveshare(EinkWaveshareAdapter),
}
