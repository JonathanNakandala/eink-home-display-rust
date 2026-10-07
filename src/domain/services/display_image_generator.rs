use crate::domain::models::GlanceData;
use crate::domain::models::display::DisplayProfile;
use crate::domain::models::image::ImageData;

#[allow(async_fn_in_trait)]
pub trait DisplayImageGenerator {
    /// Render `data` as a full-colour PNG sized for `profile`.
    async fn generate(
        &self,
        data: GlanceData,
        profile: &DisplayProfile,
    ) -> anyhow::Result<ImageData>;
}
