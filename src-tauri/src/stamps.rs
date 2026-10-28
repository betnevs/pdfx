//! Built-in stamps and their native PDF appearance.
//!
//! A stamp is a native PDFium `/Stamp` annotation whose visible content is a
//! custom normal appearance stream. Pdfium cannot synthesize a stamp
//! appearance, and its custom-appearance API installs only an opacity
//! graphics state resource, not fonts; the label glyphs are therefore baked as
//! vector outlines (see [`crate::stamp_glyphs`]) and the whole badge is drawn
//! in page space. The result renders identically in PDFium, Preview, Acrobat
//! and Chrome without embedding a CJK font.
//!
//! Geometry is exchanged with the frontend in display space (top-left origin,
//! page rotation and CropBox applied), the same system the text layer and Text
//! Box use. The PDF-space frame is stored as a centre point, width, height and
//! counter-clockwise rotation because an annotation `/Rect` is axis-aligned
//! and cannot carry rotation itself.

use serde::{Deserialize, Serialize};

use crate::commands::{CommandError, CommandErrorCode, DisplayGeometry};
use crate::pdfium_adapter::AnnotationRect;
use crate::stamp_glyphs;

/// The art box every stamp outline is designed in.
pub(crate) const ART_WIDTH: f32 = 600.0;
pub(crate) const ART_HEIGHT: f32 = 200.0;
/// Smallest stamp the transform command accepts, in display points.
pub(crate) const MIN_WIDTH: f32 = 30.0;
pub(crate) const MIN_HEIGHT: f32 = 10.0;
/// Size used by the frontend when a stamp is placed with a click.
pub const DEFAULT_WIDTH: f32 = 180.0;
pub const DEFAULT_HEIGHT: f32 = 60.0;

/// The built-in stamps PDFX can place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StampKind {
    Approved,
    Draft,
    Confidential,
    Rejected,
}

impl StampKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Approved => "已批准",
            Self::Draft => "草稿",
            Self::Confidential => "机密",
            Self::Rejected => "已拒绝",
        }
    }

    /// Stamp ink as `#rrggbb`.
    pub fn color(self) -> &'static str {
        match self {
            Self::Approved => "#15803d",
            Self::Draft => "#475569",
            Self::Confidential => "#b91c1c",
            Self::Rejected => "#7f1d1d",
        }
    }

    fn rgb(self) -> (u8, u8, u8) {
        match self {
            Self::Approved => (0x15, 0x80, 0x3d),
            Self::Draft => (0x47, 0x55, 0x69),
            Self::Confidential => (0xb9, 0x1c, 0x1c),
            Self::Rejected => (0x7f, 0x1d, 0x1d),
        }
    }

    fn art(self) -> &'static str {
        match self {
            Self::Approved => stamp_glyphs::APPROVED,
            Self::Draft => stamp_glyphs::DRAFT,
            Self::Confidential => stamp_glyphs::CONFIDENTIAL,
            Self::Rejected => stamp_glyphs::REJECTED,
        }
    }

    /// Parses the private `/PDFXStamp` value. Unknown names are ignored so a
    /// future stamp kind added by a newer build does not fail older listings.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "approved" => Some(Self::Approved),
            "draft" => Some(Self::Draft),
            "confidential" => Some(Self::Confidential),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Draft => "draft",
            Self::Confidential => "confidential",
            Self::Rejected => "rejected",
        }
    }
}

/// A stamp frame as the overlay and property panel see it: top-left origin,
/// display-space points, clockwise rotation in degrees (the same convention as
/// CSS `rotate()` and the Text Box overlay).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StampFrameView {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
}

/// One built-in stamp as the annotion list consumes it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StampView {
    pub kind: StampKind,
    pub frame: StampFrameView,
}

/// The stamp frame in PDF user space: centre, size and counter-clockwise
/// rotation. The centre representation makes page-rotation conversion
/// symmetric.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct StampFrame {
    pub(crate) center_x: f32,
    pub(crate) center_y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) rotation: f32,
}

fn invalid(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::InvalidArgument, message)
}

fn normalize_degrees(degrees: f32) -> f32 {
    let degrees = degrees % 360.0;
    if degrees < 0.0 {
        degrees + 360.0
    } else {
        degrees
    }
}

/// Validates frontend geometry before it reaches PDFium.
pub(crate) fn validate_frame(frame: &StampFrameView) -> Result<(), CommandError> {
    let finite = frame.x.is_finite()
        && frame.y.is_finite()
        && frame.width.is_finite()
        && frame.height.is_finite()
        && frame.rotation.is_finite();
    if !finite {
        return Err(invalid("stamp geometry must be finite"));
    }
    if frame.width < MIN_WIDTH || frame.height < MIN_HEIGHT {
        return Err(invalid(format!(
            "a stamp must be at least {MIN_WIDTH} by {MIN_HEIGHT} points"
        )));
    }
    Ok(())
}

impl StampFrame {
    /// An unrotated frame occupying `rect`. Used when a stamp's private frame
    /// key is missing or malformed.
    pub(crate) fn from_rect(rect: AnnotationRect) -> Self {
        Self {
            center_x: (rect.left + rect.right) / 2.0,
            center_y: (rect.bottom + rect.top) / 2.0,
            width: rect.right - rect.left,
            height: rect.top - rect.bottom,
            rotation: 0.0,
        }
    }

    pub(crate) fn from_view(
        geometry: &DisplayGeometry,
        view: StampFrameView,
    ) -> Result<Self, CommandError> {
        validate_frame(&view)?;
        let center_x = view.x + view.width / 2.0;
        let center_y = view.y + view.height / 2.0;
        let (pdf_center_x, pdf_center_y) = geometry.point_inverse(center_x, center_y);
        let radians = view.rotation.to_radians();
        let (direction_x, direction_y) = (radians.cos(), radians.sin());
        let (edge_x, edge_y) =
            geometry.point_inverse(center_x + direction_x, center_y + direction_y);
        let rotation = normalize_degrees(
            (edge_y - pdf_center_y)
                .atan2(edge_x - pdf_center_x)
                .to_degrees(),
        );
        Ok(Self {
            center_x: pdf_center_x,
            center_y: pdf_center_y,
            width: view.width,
            height: view.height,
            rotation,
        })
    }

    pub(crate) fn to_view(self, geometry: &DisplayGeometry) -> StampFrameView {
        let (center_x, center_y) = geometry.point(self.center_x, self.center_y);
        let radians = self.rotation.to_radians();
        let (edge_x, edge_y) =
            geometry.point(self.center_x + radians.cos(), self.center_y + radians.sin());
        StampFrameView {
            x: center_x - self.width / 2.0,
            y: center_y - self.height / 2.0,
            width: self.width,
            height: self.height,
            rotation: normalize_degrees((edge_y - center_y).atan2(edge_x - center_x).to_degrees()),
        }
    }

    /// The axis-aligned bounding box in PDF user space, used as `/Rect`.
    pub(crate) fn bounds(&self) -> AnnotationRect {
        let radians = self.rotation.to_radians();
        let (cos, sin) = (radians.cos().abs(), radians.sin().abs());
        let half_width = self.width / 2.0;
        let half_height = self.height / 2.0;
        let dx = half_width * cos + half_height * sin;
        let dy = half_width * sin + half_height * cos;
        AnnotationRect::new(
            self.center_y - dy,
            self.center_x - dx,
            self.center_y + dy,
            self.center_x + dx,
        )
    }

    /// Serializes the frame for the private `/PDFXStampFrame` key.
    pub(crate) fn encode(&self) -> String {
        format!(
            "{:.4} {:.4} {:.4} {:.4} {:.4}",
            self.center_x, self.center_y, self.width, self.height, self.rotation
        )
    }

    /// Parses `/PDFXStampFrame`, returning `None` for a malformed value.
    pub(crate) fn decode(value: &str) -> Option<Self> {
        let mut parts = value.split_whitespace();
        let mut next = || parts.next().and_then(|part| part.parse::<f32>().ok());
        let frame = Self {
            center_x: next()?,
            center_y: next()?,
            width: next()?,
            height: next()?,
            rotation: next()?,
        };
        if parts.next().is_some() {
            return None;
        }
        let finite = frame.center_x.is_finite()
            && frame.center_y.is_finite()
            && frame.width.is_finite()
            && frame.height.is_finite()
            && frame.rotation.is_finite();
        (finite && frame.width >= MIN_WIDTH && frame.height >= MIN_HEIGHT).then_some(frame)
    }

    /// Builds the normal appearance stream for `kind`.
    ///
    /// Pdfium interprets the stream in page coordinates with `/Rect` as the
    /// Form XObject `/BBox`, so the `cm` places the 600 x 200 art box at the
    /// frame centre, scales it to the stamp size and rotates it. `/GS` is the
    /// constant-opacity graphics state Pdfium installs with the stream.
    pub(crate) fn appearance(&self, kind: StampKind) -> String {
        let radians = self.rotation.to_radians();
        let (cos, sin) = (radians.cos(), radians.sin());
        let scale_x = self.width / ART_WIDTH;
        let scale_y = self.height / ART_HEIGHT;
        let a = scale_x * cos;
        let b = scale_x * sin;
        let c = -scale_y * sin;
        let d = scale_y * cos;
        let e = self.center_x - self.width / 2.0 * cos + self.height / 2.0 * sin;
        let f = self.center_y - self.width / 2.0 * sin - self.height / 2.0 * cos;
        let (red, green, blue) = kind.rgb();
        let red = f32::from(red) / 255.0;
        let green = f32::from(green) / 255.0;
        let blue = f32::from(blue) / 255.0;
        format!(
            "q\n/GS gs\n{a:.4} {b:.4} {c:.4} {d:.4} {e:.4} {f:.4} cm\n\
             {red:.4} {green:.4} {blue:.4} RG\n\
             {red:.4} {green:.4} {blue:.4} rg\n\
             {}\nQ\n",
            kind.art()
        )
    }
}

/// Axis-aligned bounds of a display-space frame, for the selection outline.
pub(crate) fn display_bounds(view: &StampFrameView) -> (f32, f32, f32, f32) {
    let radians = view.rotation.to_radians();
    let (cos, sin) = (radians.cos().abs(), radians.sin().abs());
    let half_width = view.width / 2.0;
    let half_height = view.height / 2.0;
    let dx = half_width * cos + half_height * sin;
    let dy = half_width * sin + half_height * cos;
    let center_x = view.x + half_width;
    let center_y = view.y + half_height;
    (center_x - dx, center_y - dy, dx * 2.0, dy * 2.0)
}
