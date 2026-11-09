//! A JPEG to one standard Image ([`read`]).
use crate::jpeg::{self, DecodeOptions, Samples};
use kinogaki_studio_api::geometry::image::{ColorTag, Encoding, Format, Image, Rect, Sample};
use kinogaki_studio_api::{Error, FileItem, Imported, Input, Reading, standard};
use std::path::Path;
use std::sync::Arc;

/// `input` as one item holding its Image, named after the file: decoded
/// within the read's budget (its coefficients and pixels charged before
/// they are allocated, the Image too), stopping when it is cancelled.
pub(crate) fn read(input: &Input<'_>, reading: &Reading<'_>) -> Result<Imported, Error> {
    reading.check()?;
    let bytes = input.bytes()?;
    let at = |e: jpeg::Error| match e {
        jpeg::Error::Cancelled => Error::cancelled(),
        e => Error::invalid(format!("{input}: {e}")),
    };
    let options = DecodeOptions {
        budget: reading.budget.clone(),
        ..Default::default()
    };
    let decoded = jpeg::decode(&bytes, &options).map_err(at)?;
    // The Image's copy of the pixels.
    let held = u64::from(decoded.width) * u64::from(decoded.height);
    let held = held * u64::from(decoded.channels);
    reading.budget.allocate(held).map_err(|e| at(e.into()))?;
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
        ..Imported::default()
    })
}
