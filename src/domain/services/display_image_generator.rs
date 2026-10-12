use crate::domain::models::GlanceData;
use crate::domain::models::display::DisplayProfile;
use crate::domain::models::image::ImageData;
use crate::domain::models::layout::LayoutName;

#[allow(async_fn_in_trait)]
pub trait DisplayImageGenerator {
    /// Render `data` as a full-colour PNG sized for `profile`, laid out as `layout`.
    async fn generate(
        &self,
        data: GlanceData,
        profile: &DisplayProfile,
        layout: &LayoutName,
    ) -> anyhow::Result<ImageData>;
}
