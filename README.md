# JPEG

JPEG as a format plugin for [Kinogaki Studio](https://github.com/kinogaki): a JPEG reads as one standard Image of the public [studio-sdk](https://github.com/kinogaki/studio-sdk), and an Image writes as one baseline JPEG. The codec is this crate's own, `src/jpeg/` ([docs/CODEC.md](docs/CODEC.md)): baseline and progressive decoding, baseline encoding, ICC profiles, its parallel steps on the process's workers. The format converts no color: the host resolves what a file says of its colors.

The library crate is `kinogaki-jpeg-format`; `plugin/` (`kinogaki-plugin-jpeg`) is the dylib Studio loads, `formats/jpeg.dylib`. Package `kinogaki.jpeg`, format `kinogaki.format.jpeg` (`.jpg`, `.jpeg`).

## Reading

One layer, `""`: `Y` or `R G B` of 8-bit samples (CMYK and YCCK converted to RGB), tagged with the file's ICC profile, else nothing said. A file in memory (an embedded one) reads as one on disk.

A file is read as the SDK's policy for untrusted files says (the SDK's [Your first file format](https://github.com/kinogaki/studio-sdk/blob/main/docs/FIRST-FORMAT.md#reading-untrusted-files)): its size (the budget's pixels, 2²⁸ by default), its coefficients (refused when the frame codes more blocks than the file has bits for, a bit a block at least) and its pixels are checked against the read's budget before they are allocated, as is the Image, and the decode polls the read's cancellation every row of blocks.

## Writing

The main layer (`""`, else the first) as 8-bit grey or color at the `quality` (1–100) and chroma `subsampling` (4:4:4, 4:2:2, 4:2:0) the options name; colors premultiplied by alpha are divided by it, then alpha is left out. A profile the host tags it with is embedded; the sRGB display is JPEG's own assumption and needs none. Alpha, other layers, a tag a JPEG can't hold and further images are left out, each said once. The writer is display-referred: a host gives it a display's code values.

## Code

| File | Role |
|---|---|
| `src/lib.rs` | The package and the format |
| `src/read.rs` | A JPEG to one Image |
| `src/write.rs` | One Image to a JPEG, and the writer's options |
| `src/jpeg/` | The codec, knowing no SDK but the process's workers and the read budget ([docs/CODEC.md](docs/CODEC.md)) |

## Tests and data

- `tests/decode.rs`, `tests/encode.rs`: the codec against libjpeg-turbo ([docs/CODEC.md](docs/CODEC.md#tests-and-oracles)).
- `tests/hostile.rs`: hostile files made in code, read as Import reads them, each a clean error or a picture within seconds: a progressive refinement past 16 bits (once a panic), a frame larger than its file can code, a picture past a small budget, a cancelled read.
- `tests/format.rs`: files read as the codec decodes them on any number of workers, tagged with their profile; an Image written as the codec encodes it, with its profile; alpha and a named space said and left out; damaged and truncated files fail without panicking.
- `examples/jpeg_bench.rs` times the codec.

The fixtures are synthetic pictures we encoded with Pillow, libjpeg-turbo's cjpeg and TurboJPEG, and libjpeg-turbo's reference decodes of them (`tools/jpeg_oracle.py`); two ICC files are from Ladybird's test inputs under the BSD 2-Clause License ([tests/fixtures/icc/LICENSE-ladybird](tests/fixtures/icc/LICENSE-ladybird), [NOTICE](tests/fixtures/icc/NOTICE)). The oracle runs locally (Pillow, so libjpeg-turbo) and is never a dependency.

## Building

The format builds from this repository, the public studio-sdk and lib-image (its interleaved `Image`; its PNG reads the reference decodes in the tests), checked out side by side as Studio's tree lays them out (`sdk/`, `libs/lib-image/` and `plugins/jpeg/`), with the compiler the SDK pins (`sdk/rust-toolchain.toml`):

```sh
cd plugins/jpeg
cargo test
```

`plugin/` links Studio's shared library (`kinogaki_shared`), so it is built in Studio's tree, or against a Studio release's plugin kit as the SDK's guides describe. Studio's tree also runs the image formats' workflows (`workflows/`).

## License

Dual-licensed under the MIT license ([LICENSE-MIT](LICENSE-MIT)) and the Apache License 2.0 ([LICENSE-APACHE](LICENSE-APACHE)), at your option.
