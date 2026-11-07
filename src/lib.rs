//! JPEG, a format plugin: a JPEG reads as one standard
//! [`Image`](kinogaki_studio_api::geometry::image::Image) as the file holds
//! it, and an Image writes as one baseline JPEG. The codec is [`jpeg`],
//! which knows no SDK but the process's workers it runs on.
//!
//! - **Reading.** One layer, `""`: `Y` or `R G B` of 8-bit samples (CMYK
//!   and YCCK converted to RGB), tagged with the file's ICC profile, else
//!   nothing said.
//! - **Writing.** The main layer (`""`, else the first) as 8-bit grey or
//!   color, its color divided by its alpha when premultiplied, at the
//!   `quality` and chroma `subsampling` the options name; a profile is
//!   embedded, the sRGB display is JPEG's own assumption. Alpha, other
//!   layers and a tag a JPEG can't hold are left out with a warning.
pub mod jpeg;
mod read;
mod write;

use kinogaki_studio_api::{
    DataTypeId, Error, FileFormat, FormatId, FormatOptions, FormatWriter, Imported, Input, Package,
    PackageId, Reading, standard,
};

/// The package's id.
pub fn id() -> PackageId {
    PackageId::declared("kinogaki.jpeg")
}

/// The format's id.
pub fn format() -> FormatId {
    FormatId::declared("kinogaki.format.jpeg")
}

/// The package: the format alone.
pub fn package() -> Package {
    Package::new(id(), "JPEG").with_format(Jpeg)
}

/// What it reads and writes: the standard Image.
const IMAGE: &[DataTypeId] = &[standard::image().id()];

/// The format.
pub struct Jpeg;

impl FileFormat for Jpeg {
    fn id(&self) -> FormatId {
        format()
    }
    fn label(&self) -> &str {
        "JPEG"
    }
    fn extensions(&self) -> &[&str] {
        &["jpg", "jpeg"]
    }
    fn detects(&self, head: &[u8]) -> bool {
        head.starts_with(&[0xFF, 0xD8, 0xFF])
    }
    fn produces(&self) -> &[DataTypeId] {
        IMAGE
    }
    fn read(
        &self,
        input: &Input<'_>,
        _: &FormatOptions,
        reading: &Reading<'_>,
    ) -> Result<Imported, Error> {
        read::read(input, reading)
    }
    fn writer(&self) -> Option<&dyn FormatWriter> {
        Some(&write::Writer)
    }
}
