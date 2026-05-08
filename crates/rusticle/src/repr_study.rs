use crate::error::Result;
use crate::types::{Frame, Gif};

pub use crate::voyager_exact_bbox_global_palette::{
    VoyagerExactBboxGlobalPaletteBuilder, VoyagerExactBboxGlobalPaletteFrame,
    VoyagerExactBboxGlobalPaletteRepr,
};
pub use crate::voyager_exact_bbox_global_palette_with_fallback::{
    VoyagerExactBboxGlobalPaletteFallbackBuilder, VoyagerExactBboxGlobalPaletteFallbackFrame,
    VoyagerExactBboxGlobalPaletteFallbackRepr,
};
pub use crate::voyager_repr::{VoyagerBuilder, VoyagerFrame, VoyagerRepr};
pub use crate::voyager_source_reuse::{
    SourceReuseViability, VoyagerSourceReuseBuilder, VoyagerSourceReuseFrame,
    VoyagerSourceReuseRepr,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReprStrategy {
    FullFrameGlobalPalette,
    OpaqueBboxSourcePalette,
    OpaqueBboxDerivedGlobal,
    OpaqueBboxDerivedGlobalWithFallback { threshold: f64 },
}

pub enum ReprStudyOutput {
    FullFrame(VoyagerRepr),
    SourceReuse(VoyagerSourceReuseRepr),
    DerivedGlobal(VoyagerExactBboxGlobalPaletteRepr),
    DerivedGlobalWithFallback(VoyagerExactBboxGlobalPaletteFallbackRepr),
}

impl VoyagerBuilder {
    pub fn build_with_strategy(
        resized_frames: &[Frame],
        canvas_width: u16,
        canvas_height: u16,
        strategy: ReprStrategy,
        source_gif: Option<&Gif>,
    ) -> Result<ReprStudyOutput> {
        match strategy {
            ReprStrategy::FullFrameGlobalPalette => Ok(ReprStudyOutput::FullFrame(Self::build(
                resized_frames,
                canvas_width,
                canvas_height,
            )?)),
            ReprStrategy::OpaqueBboxSourcePalette => Ok(ReprStudyOutput::SourceReuse(
                VoyagerSourceReuseBuilder::build(
                    resized_frames,
                    canvas_width,
                    canvas_height,
                    source_gif.ok_or_else(|| {
                        crate::error::Error::EncodeError(
                            "source GIF required for source palette strategy".to_string(),
                        )
                    })?,
                )?,
            )),
            ReprStrategy::OpaqueBboxDerivedGlobal => Ok(ReprStudyOutput::DerivedGlobal(
                VoyagerExactBboxGlobalPaletteBuilder::build(
                    resized_frames,
                    canvas_width,
                    canvas_height,
                )?,
            )),
            ReprStrategy::OpaqueBboxDerivedGlobalWithFallback { threshold } => {
                Ok(ReprStudyOutput::DerivedGlobalWithFallback(
                    VoyagerExactBboxGlobalPaletteFallbackBuilder::build(
                        resized_frames,
                        canvas_width,
                        canvas_height,
                        threshold,
                    )?,
                ))
            }
        }
    }
}
