//! JPEG as a Studio plugin library (`formats/jpeg.dylib`).
use kinogaki_studio_api::export_package;

kinogaki_shared::link!();
export_package!(kinogaki_jpeg_format::package);
