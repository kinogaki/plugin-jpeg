//! A JPEG to one standard Image ([`read`]).
use crate::jpeg::{self, DecodeOptions, Samples};
use kinogaki_studio_api::geometry::image::{ColorTag, Encoding, Format, Image, Rect, Sample};
use kinogaki_studio_api::{Error, FileItem, Imported, Input, Reading, standard};
use std::path::Path;
use std::sync::Arc;

/// The most pixels a file may have (2²⁸).
const MAX_PIXELS: u64 = 1 << 28;

/// `input` as one item holding its Image, named after the file.
pub(crate) fn read(input: &Input<'_>, reading: &Reading<'_>) -> Result<Imported, Error> {
    reading.check()?;
    let bytes = input.bytes()?;
    let at = |e: jpeg::Error| Error::invalid(format!("{input}: {e}"));
    let options = DecodeOptions {
        max_pixels: MAX_PIXELS,
        ..Default::default()
    };
    let decoded = jpeg::decode(&bytes, &options).map_err(at)?;
    let tag = match jpeg::icc_profile(&bytes).map_err(at)? {
        Some(profile) => ColorTag::Icc(profile.into()),
        None => ColorTag::Unknown { linear: false },
    };
    reading.check()?;
    let Samples::U8(pixels) = decoded.samples else {
        return Err(Error::invalid(format!("{input}: not 8-bit samples")));
    };
    let names: &[&str] = match decoded.channels {
        1 => &["Y"],
        _ => &["R", "G", "B"],
    };
    let format = Format {
        name: "".into(),
        channels: names.iter().map(|&n| Arc::from(n)).collect(),
        sample: Sample::U8,
        encoding: Encoding::Color {
            tag,
            premultiplied: false,
        },
    };
    let window = Rect::new([0, 0], [decoded.width as i32, decoded.height as i32])?;
    let image = Image::builder(window).plane(format, &pixels).build()?;
    let name = (Path::new(input.name()).file_stem())
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    Ok(Imported {
        items: vec![FileItem::top(name, standard::image().wrap(image))],
        warnings: Vec::new(),
    })
}
