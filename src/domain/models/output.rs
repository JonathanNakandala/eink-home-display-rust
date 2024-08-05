use std::path::PathBuf;
#[derive(derive_new::new)]
pub struct OutputResult {
    pub output_path: PathBuf,
}
