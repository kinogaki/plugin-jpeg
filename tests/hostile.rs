//! Hostile JPEGs, each made here and read as File ▸ Import reads one (on its
//! 2 MiB stack, within a budget): a clean error, quickly.
use kinogaki_jpeg_format::Jpeg;
use kinogaki_jpeg_format::jpeg::{self, EncodeOptions};
use kinogaki_studio_api::format::{ReadBudget, ReadLimits};
use kinogaki_studio_api::{
    CancelToken, Error, FileFormat, Imported, Input, Purpose, Reading, Units,
};
use kinogaki_studio_api_testing::{Unconverted, read_hostile};
use std::time::Duration;

/// What a read of a hostile file may take, in a debug build.
const QUICK: Duration = Duration::from_secs(5);

fn read(bytes: &[u8], limits: ReadLimits) -> Result<Imported, Error> {
    let input = Input::memory("hostile.jpg", bytes);
    let (read, took) = read_hostile(&Jpeg, &input, &Jpeg.options(), limits);
    assert!(took < QUICK, "{took:?}");
    read
}

/// A marker segment: `code`, its length, `payload`.
fn segment(out: &mut Vec<u8>, code: u8, payload: &[u8]) {
    out.extend_from_slice(&[0xFF, code]);
    out.extend_from_slice(&(payload.len() as u16 + 2).to_be_bytes());
    out.extend_from_slice(payload);
}

/// A progressive greyscale frame header of `width` × `height` pixels and
/// `components` components, after SOI and a quantization table of ones.
fn progressive(width: u16, height: u16, components: u8) -> Vec<u8> {
    let mut file = vec![0xFF, 0xD8];
    segment(&mut file, 0xDB, &[[0u8].as_slice(), &[1; 64]].concat());
    let mut frame = vec![8];
    frame.extend_from_slice(&height.to_be_bytes());
    frame.extend_from_slice(&width.to_be_bytes());
    frame.push(components);
    for id in 1..=components {
        frame.extend_from_slice(&[id, 0x11, 0]);
    }
    segment(&mut file, 0xC2, &frame);
    file
}

/// A Huffman table `class << 4 | id` of `symbols`, the first one bit long,
/// the next two.
fn table(file: &mut Vec<u8>, class_id: u8, symbols: &[u8]) {
    let mut counts = [0u8; 16];
    counts[0] = 1;
    counts[1] = symbols.len() as u8 - 1;
    segment(file, 0xC4, &[&[class_id][..], &counts, symbols].concat());
}

/// A scan of component 1 with tables `dc << 4 | ac`, coefficients `ss..=se`
/// at approximation `ah`, `al`, its coded data `data`.
fn scan(file: &mut Vec<u8>, tables: u8, (ss, se): (u8, u8), (ah, al): (u8, u8), data: &[u8]) {
    segment(file, 0xDA, &[1, 1, tables, ss, se, ah << 4 | al]);
    file.extend_from_slice(data);
}

#[test]
fn a_refinement_past_sixteen_bits_wraps_as_libjpeg_does() {
    // A first AC scan leaves -16384 << 1 = -32768 in a coefficient, and its
    // refinement takes one more from it: once a panic of 133 bytes.
    let mut file = progressive(8, 8, 1);
    table(&mut file, 0x00, &[0x00]); // DC: difference 0 in `0`
    table(&mut file, 0x10, &[0x0F, 0x00]); // AC: a 15-bit value `0`, end `10`
    table(&mut file, 0x11, &[0x00, 0x01]); // refinement: end `0`
    scan(&mut file, 0x00, (0, 0), (0, 0), &[0x7F]);
    // `0`, then -16384 as 15 bits (16383): 0x3F 0xFF, stuffed.
    scan(&mut file, 0x00, (1, 1), (0, 1), &[0x3F, 0xFF, 0x00]);
    // End of band `0`, then a correction bit `1` for the coefficient.
    scan(&mut file, 0x01, (1, 1), (1, 0), &[0x7F]);
    file.extend_from_slice(&[0xFF, 0xD9]);
    assert!(read(&file, ReadLimits::default()).is_ok());
}

#[test]
fn a_frame_larger_than_its_file_can_code_is_refused_before_its_coefficients() {
    // 16384² pixels of three components from a hundred bytes: its
    // coefficients once took 1.6 GB at the first scan's header.
    let mut file = progressive(16384, 16384, 3);
    table(&mut file, 0x00, &[0x00]);
    segment(&mut file, 0xDA, &[3, 1, 0, 2, 0, 3, 0, 0, 0, 0]);
    file.extend_from_slice(&[0x7F, 0xFF, 0xD9]);
    let why = read(&file, ReadLimits::default()).unwrap_err();
    assert!(why.message().contains("over 1032 times its size"), "{why}");
}

#[test]
fn a_picture_past_the_budget_is_refused_before_it_decodes() {
    let pixels: Vec<u8> = (0..1024 * 1024 * 4).map(|i| (i % 251) as u8).collect();
    let image = jpeg::Image::rgba8(1024, 1024, pixels);
    let file = jpeg::encode(&image, &EncodeOptions::default()).unwrap();
    let small = ReadLimits {
        allocation: 1 << 20,
        total: 1 << 20,
        ..ReadLimits::default()
    };
    let why = read(&file, small).unwrap_err();
    assert!(why.message().contains("over the read's budget"), "{why}");
    assert!(read(&file, ReadLimits::default()).is_ok());
}

#[test]
fn a_cancelled_read_stops() {
    let image = jpeg::Image::rgba8(256, 256, vec![90; 256 * 256 * 4]);
    let file = jpeg::encode(&image, &EncodeOptions::default()).unwrap();
    let token = CancelToken::default();
    token.cancel();
    let reading = Reading {
        budget: ReadBudget::default().cancelled_by(token),
        ..Reading::new(Purpose::Value, Units::default(), &Unconverted)
    };
    let read = Jpeg.read(&Input::memory("a.jpg", &file), &Jpeg.options(), &reading);
    assert_eq!(read.unwrap_err(), Error::cancelled());
}
