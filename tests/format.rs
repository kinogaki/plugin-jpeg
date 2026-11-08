//! The format through the SDK's contract: a file read as the codec
//! decodes it and tagged with its profile, an Image written as the codec
//! encodes it, and damaged files.

use kinogaki_jpeg_format::Jpeg;
use kinogaki_jpeg_format::jpeg::{self, DecodeOptions, EncodeOptions, Samples};
use kinogaki_studio_api::color::SRGB_DISPLAY;
use kinogaki_studio_api::geometry::image::{ColorTag, Encoding, Format, Image, Rect, Sample};
use kinogaki_studio_api::{
    FileFormat, FileItem, Imported, Input, Purpose, Reading, Units, Value, Writing, standard,
};
use kinogaki_studio_api_testing::Unconverted;
use kinogaki_studio_api_testing::format::Scratch;
use kinogaki_studio_api_testing::workers;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read(input: &Input<'_>) -> Result<Imported, kinogaki_studio_api::Error> {
    let colors = Unconverted;
    let reading = Reading::new(Purpose::Value, Units::default(), &colors);
    Jpeg.read(input, &Jpeg.options(), &reading)
}

fn image(input: &Input<'_>) -> Image {
    let read = read(input).unwrap_or_else(|e| panic!("{input}: {e}"));
    standard::image()
        .get(read.items[0].data.as_ref().unwrap())
        .unwrap()
        .clone()
}

/// `image` written with `quality`: the file's bytes and the warnings.
fn written(image: &Image, quality: i64) -> (Vec<u8>, Vec<String>) {
    let writer = Jpeg.writer().unwrap();
    let mut options = writer.options();
    options.insert("quality".into(), Value::Integer(quality));
    let dir = Scratch::new("jpeg-format");
    let path = dir.join("picture.jpg");
    let colors = Unconverted;
    let item = FileItem::top("picture", standard::image().wrap(image.clone()));
    let warnings = writer
        .write(
            &path,
            &[item],
            &Writing::new(Units::default(), &colors),
            &options,
        )
        .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    (bytes, warnings.into_iter().map(|w| w.message).collect())
}

/// A `width` × 1 Image of one 8-bit layer.
fn picture(channels: &[&str], tag: ColorTag, bytes: &[u8]) -> Image {
    let format = Format {
        name: "".into(),
        channels: channels.iter().map(|&c| Arc::from(c)).collect(),
        sample: Sample::U8,
        encoding: Encoding::Color {
            tag,
            premultiplied: false,
        },
    };
    let width = bytes.len() / channels.len();
    let window = Rect::new([0, 0], [width as i32, 1]).unwrap();
    Image::builder(window).plane(format, bytes).build().unwrap()
}

#[test]
fn a_file_reads_as_the_codec_decodes_it_on_any_workers() {
    for name in [
        "b420_37x35.jpg",
        "gray_64x49.jpg",
        "cmyk_37x35.jpg",
        "p444_64x49.jpg",
        "mt_restart.jpg",
    ] {
        let bytes = std::fs::read(fixtures().join(name)).unwrap();
        let decoded = jpeg::decode(&bytes, &DecodeOptions::default()).unwrap();
        let Samples::U8(pixels) = &decoded.samples else {
            panic!("8 bits")
        };
        workers::install(1);
        let one = image(&Input::memory(name, &bytes));
        workers::install(6);
        let many = image(&Input::file(&fixtures().join(name)));
        assert_eq!(one, many, "{name}");
        assert_eq!(one.plane(0).unwrap(), *pixels, "{name}");
        let channels: Vec<&str> = one.layers()[0]
            .format()
            .channels
            .iter()
            .map(|c| &**c)
            .collect();
        assert_eq!(
            channels,
            if decoded.channels == 1 {
                vec!["Y"]
            } else {
                vec!["R", "G", "B"]
            },
            "{name}"
        );
        let tag = &one.layers()[0].format().encoding;
        assert_eq!(
            *tag,
            Encoding::Color {
                tag: ColorTag::Unknown { linear: false },
                premultiplied: false
            }
        );
    }
    let profiled = image(&Input::file(&fixtures().join("icc/icc-one.jpg")));
    let Encoding::Color {
        tag: ColorTag::Icc(profile),
        ..
    } = &profiled.layers()[0].format().encoding
    else {
        panic!("a profile");
    };
    assert!(!profile.is_empty());
}

#[test]
fn an_image_writes_as_the_codec_encodes_it_with_its_profile() {
    let rgb: Vec<u8> = (0..48).map(|i| (i * 5) as u8).collect();
    let srgb = picture(&["R", "G", "B"], ColorTag::Space(SRGB_DISPLAY.into()), &rgb);
    let (file, warnings) = written(&srgb, 75);
    assert!(warnings.is_empty(), "{warnings:?}");
    let codec = jpeg::Image::new(16, 1, 3, Samples::U8(rgb.clone())).unwrap();
    let options = EncodeOptions {
        quality: 75,
        ..Default::default()
    };
    assert_eq!(file, jpeg::encode(&codec, &options).unwrap());
    let back = image(&Input::memory("back.jpg", &file));
    let close = (back.plane(0).unwrap().iter().zip(&rgb)).all(|(a, b)| a.abs_diff(*b) <= 12);
    assert!(close);
    let profile = vec![3u8; 300];
    let tagged = picture(
        &["R", "G", "B"],
        ColorTag::Icc(profile.clone().into()),
        &rgb,
    );
    let (file, _) = written(&tagged, 90);
    assert_eq!(jpeg::icc_profile(&file).unwrap(), Some(profile));
}

#[test]
fn alpha_and_a_named_space_are_said_and_left_out() {
    let rgba = picture(
        &["R", "G", "B", "A"],
        ColorTag::Space("ACEScg".into()),
        &[10, 20, 30, 255, 40, 50, 60, 128],
    );
    let (file, warnings) = written(&rgba, 90);
    assert_eq!(
        warnings,
        [
            "a JPEG holds no alpha: left out",
            "a JPEG can't say its colors are ACEScg: written untagged"
        ]
    );
    assert_eq!(jpeg::info(&file).unwrap().components, 3);
    assert_eq!(jpeg::icc_profile(&file).unwrap(), None);
}

#[test]
fn a_damaged_or_truncated_file_is_an_error_naming_it() {
    let bytes = std::fs::read(fixtures().join("b420_64x49.jpg")).unwrap();
    for cut in [0, 2, 40, bytes.len() / 2] {
        let why = read(&Input::memory("cut.jpg", &bytes[..cut])).unwrap_err();
        assert!(why.message().starts_with("cut.jpg: "), "{why}");
    }
    assert!(Jpeg.detects(&bytes[..3]) && !Jpeg.detects(b"\x89PNG"));
}
