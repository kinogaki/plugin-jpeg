//! Times the codec decoding and encoding on as many threads as `THREADS`
//! says (the tests' workers; unset or 0, every processor):
//! `jpeg_bench decode FILE [repeats] [scale] [rgba]` and
//! `jpeg_bench encode FILE [repeats] [quality] [444|422|420] [r|o|ro|-]`
//! (encoding the file's own decoded pixels). Prints the best of the runs.
//! `jpeg_bench dump FILE OUT [scale]` writes the RGB decode (width, height, 3 as
//! little-endian u32, then the bytes), for comparing with another decoder.
#[path = "../tests/common/mod.rs"]
mod common;

use kinogaki_jpeg_format::jpeg::{DecodeOptions, EncodeOptions, Subsampling};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let number =
        |i: usize, fallback: usize| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(fallback);
    let data = std::fs::read(&args[2]).expect("file");
    let wanted = std::env::var("THREADS").ok().and_then(|t| t.parse().ok());
    let threads = match wanted.unwrap_or(0) {
        0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
        n => n,
    };
    common::workers(threads);
    let repeats = number(3, 5);
    let mut best = f64::MAX;
    match args[1].as_str() {
        "decode" => {
            let options = DecodeOptions {
                scale: number(4, 1) as u8,
                ..Default::default()
            };
            let rgba = args.get(5).is_some_and(|s| s == "rgba");
            for _ in 0..repeats {
                let start = Instant::now();
                let image = if rgba {
                    kinogaki_jpeg_format::jpeg::decode_rgba8(&data, &options)
                } else {
                    kinogaki_jpeg_format::jpeg::decode(&data, &options)
                }
                .unwrap();
                best = best.min(start.elapsed().as_secs_f64() * 1e3);
                std::hint::black_box(image);
            }
            println!("decode {}: best {best:.1} ms of {repeats}", args[2]);
        }
        "encode" => {
            let image =
                kinogaki_jpeg_format::jpeg::decode(&data, &DecodeOptions::default()).unwrap();
            let flags = args.get(6).map_or("-", |s| s.as_str());
            let options = EncodeOptions {
                quality: number(4, 90) as u8,
                subsampling: match args.get(5).map_or("420", |s| s.as_str()) {
                    "444" => Subsampling::S444,
                    "422" => Subsampling::S422,
                    _ => Subsampling::S420,
                },
                restart: flags.contains('r'),
                optimize: flags.contains('o'),
                icc_profile: None,
            };
            let mut size = 0;
            for _ in 0..repeats {
                let start = Instant::now();
                let jpeg = kinogaki_jpeg_format::jpeg::encode(&image, &options).unwrap();
                best = best.min(start.elapsed().as_secs_f64() * 1e3);
                size = jpeg.len();
            }
            println!(
                "encode {}: best {best:.1} ms of {repeats}, {size} bytes",
                args[2]
            );
        }
        "dump" => {
            let options = DecodeOptions {
                scale: number(4, 1) as u8,
                ..Default::default()
            };
            let image = kinogaki_jpeg_format::jpeg::decode_rgba8(&data, &options).unwrap();
            let mut out = Vec::new();
            for value in [image.width, image.height, 3] {
                out.extend_from_slice(&value.to_le_bytes());
            }
            out.extend(
                image
                    .to_rgba8()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| [p[0], p[1], p[2]]),
            );
            std::fs::write(&args[3], out).unwrap();
        }
        _ => panic!("decode, encode or dump"),
    }
}
