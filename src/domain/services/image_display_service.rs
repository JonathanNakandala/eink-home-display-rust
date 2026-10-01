use async_trait::async_trait;

use crate::domain::models::display::DisplayProfile;
use crate::domain::models::image::ImageData;

#[async_trait]
pub trait ImageDisplayService {
    /// The size and colour depth this display wants images rendered for.
    fn profile(&self) -> DisplayProfile;

    /// Show a full-colour image rendered for `profile()`, converting it to the panel's format.
    async fn display(&self, data: &ImageData) -> anyhow::Result<()>;
}
