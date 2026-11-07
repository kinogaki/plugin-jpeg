//! One standard Image to a JPEG ([`Writer`]).
use crate::IMAGE;
use crate::jpeg::{self, EncodeOptions, Samples, Subsampling};
use kinogaki_studio_api::color::SRGB_DISPLAY;
use kinogaki_studio_api::format::options;
use kinogaki_studio_api::geometry::image::{ColorTag, Encoding, Image};
use kinogaki_studio_api::params::{Parameter, Schema, ValueKind};
use kinogaki_studio_api::{
    DataTypeId, Diagnostic, Error, FileItem, FormatOptions, FormatWriter, Value, Writing, standard,
};
use std::path::Path;

/// The option naming the quality, 1 to 100.
const QUALITY: &str = "quality";
/// The option naming the chroma's resolution (`444`, `422`, `420`).
const SUBSAMPLING: &str = "subsampling";

/// The writer.
pub(crate) struct Writer;

impl FormatWriter for Writer {
    fn accepts(&self) -> &[DataTypeId] {
        std::slice::from_ref(&*IMAGE)
    }
    /// Its files hold a display's code values.
    fn display_referred(&self) -> bool {
        true
    }
    fn options(&self) -> FormatOptions {
        FormatOptions::from([
            (QUALITY.to_owned(), Value::Integer(90)),
            (SUBSAMPLING.to_owned(), Value::Choice("420".into())),
        ])
    }
    fn schema(&self, chosen: &FormatOptions) -> Schema {
        let quality = options::option(chosen, QUALITY, Value::integer).unwrap_or(90);
        let subsampling = options::option(chosen, SUBSAMPLING, Value::choice).unwrap_or_default();
        let chroma = [
            ("444", "4:4:4 (Full)"),
            ("422", "4:2:2 (Half Across)"),
            ("420", "4:2:0 (Half Across and Down)"),
        ];
        Schema::default()
            .parameter(Parameter::new(
                QUALITY,
                "Quality",
                ValueKind::integer(1, 100),
                Value::Integer(quality),
            ))
            .parameter(Parameter::choice(
                SUBSAMPLING,
                "Chroma",
                &chroma,
                subsampling,
            ))
    }
    /// The first item's Image (see the crate docs); what else the items
    /// hold is left out with a warning.
    fn write(
        &self,
        path: &Path,
        items: &[FileItem],
        writing: &Writing<'_>,
        chosen: &FormatOptions,
    ) -> Result<Vec<Diagnostic>, Error> {
        writing.check()?;
        let quality = options::option(chosen, QUALITY, Value::integer)?.clamp(1, 100) as u8;
        let subsampling = match options::option(chosen, SUBSAMPLING, Value::choice)? {
            "444" => Subsampling::S444,
            "422" => Subsampling::S422,
            _ => Subsampling::S420,
        };
        let mut images =
            (items.iter().filter_map(|i| i.data.as_ref())).filter_map(|d| standard::image().get(d));
        let image = images
            .next()
            .ok_or_else(|| Error::invalid("no image to write"))?;
        let mut warnings = Vec::new();
        let more = images.count();
        if more > 0 {
            warnings.push(Diagnostic::warning(format!(
                "a JPEG holds one image: {more} more left out"
            )));
        }
        let at = (image
            .layers()
            .iter()
            .position(|l| l.format().name.is_empty()))
        .unwrap_or(0);
        if image.layers().len() > 1 {
            warnings.push(Diagnostic::warning(format!(
                "a JPEG holds one layer: {} more left out",
                image.layers().len() - 1
            )));
        }
        let picture = pixels(image, at, &mut warnings)?;
        let options = EncodeOptions {
            quality,
            subsampling,
            icc_profile: profile(image, at, &mut warnings),
            ..Default::default()
        };
        let named = |e: jpeg::Error| Error::invalid(format!("{}: {e}", path.display()));
        let bytes = jpeg::encode(&picture, &options).map_err(named)?;
        writing.check()?;
        std::fs::write(path, bytes)
            .map_err(|e| Error::invalid(format!("{}: {e}", path.display())))?;
        Ok(warnings)
    }
}

/// Layer `at` of `image` as 8-bit grey or RGB, straight; its alpha left
/// out, said.
fn pixels(image: &Image, at: usize, warnings: &mut Vec<Diagnostic>) -> Result<jpeg::Image, Error> {
    let format = image.layers()[at].format();
    let plane = image.plane(at).ok_or_else(|| {
        Error::invalid("an image whose pixels the host holds isn't written; read it back first")
    })?;
    let channels = format.channels.len();
    let alpha = channels == 2 || channels == 4;
    if alpha {
        warnings.push(Diagnostic::warning("a JPEG holds no alpha: left out"));
    }
    let premultiplied = alpha
        && matches!(
            format.encoding,
            Encoding::Color {
                premultiplied: true,
                ..
            }
        );
    let (sample, size) = (format.sample, format.sample.bytes());
    let colors = channels - usize::from(alpha);
    let mut bytes = Vec::with_capacity(plane.len() / size / channels * colors);
    for pixel in plane.chunks_exact(channels * size) {
        let a = sample.read(&pixel[(channels - 1) * size..]);
        let by = match premultiplied && a > 0.0 {
            true => 1.0 / a,
            false => 1.0,
        };
        for k in 0..colors {
            let value = sample.read(&pixel[k * size..]) * by;
            bytes.push((value.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    let [width, height] = image.data().size();
    let made = jpeg::Image::new(width, height, colors as u8, Samples::U8(bytes));
    made.map_err(|e| Error::invalid(e.to_string()))
}

/// The profile layer `at`'s tag embeds; none for the sRGB display (what a
/// JPEG is taken to be), data, or a tag a JPEG can't hold, which is said.
fn profile(image: &Image, at: usize, warnings: &mut Vec<Diagnostic>) -> Option<Vec<u8>> {
    let Encoding::Color { tag, .. } = &image.layers()[at].format().encoding else {
        return None;
    };
    match tag {
        ColorTag::Icc(profile) => Some(profile.to_vec()),
        ColorTag::Space(space) if &**space == SRGB_DISPLAY => None,
        ColorTag::Unknown { .. } => None,
        held => {
            warnings.push(Diagnostic::warning(format!(
                "a JPEG can't say its colors are {held}: written untagged"
            )));
            None
        }
    }
}
