use async_trait::async_trait;

use crate::domain::models::display::DisplayProfile;
use crate::domain::models::image::ImageData;
use crate::domain::models::layout::LayoutName;

#[async_trait]
pub trait ImageDisplayService {
    /// The size and colour depth this display wants images rendered for.
    fn profile(&self) -> DisplayProfile;

    /// Whether every layout is wanted, because displays fetch them from the server. A panel that is driven
    /// directly shows one picture, so it is given only the default layout.
    fn wants_every_layout(&self) -> bool {
        false
    }

    /// Show a full-colour image rendered for `profile()` as `layout`, converting it to the panel's format.
    async fn display(&self, layout: &LayoutName, data: &ImageData) -> anyhow::Result<()>;
}
