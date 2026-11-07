# The JPEG codec

`src/jpeg/` reads and writes JPEG files (ITU-T T.81 | ISO/IEC 10918-1, JFIF, Adobe's APP14 and the ICC's APP2 conventions) into and from lib-image's interleaved `Image`. It knows nothing of the SDK but the process's workers it runs its parallel steps on (`parallel::map`) and the read's budget.

## Coverage

Decoding:
- 8-bit Huffman JPEG: baseline (SOF0), extended sequential (SOF1), progressive (SOF2) with spectral selection and successive approximation.
- Restart intervals, decoded in parallel when the picture is one scan.
- Any sampling factors 1–4 (up to ten blocks an MCU); chroma upsampled by bilinear interpolation at pixel centres (libjpeg's triangle filter at 2:1).
- Greyscale, YCbCr, RGB (component IDs `R`, `G`, `B` or Adobe transform 0), CMYK, Adobe-inverted CMYK and YCCK (Adobe transform 2), converted to RGB.
- Output as the picture's channels (1 or 3) or RGBA; reduced decodes at 1/2, 1/4 and 1/8 straight from the DCT.
- ICC profiles from APP2 segments, joined as libjpeg-turbo joins them.
- Truncated or corrupt files fail with the byte offset of the problem. A decode takes a read budget (`DecodeOptions::budget`): the pixels are charged at the frame header, the coefficients at the first scan's (refused when the frame codes more blocks than the file holds bits, at least one a block), and the entropy decoding polls its cancellation every row of blocks. A refinement past 16 bits wraps, as libjpeg's 16-bit coefficients do.

Encoding: baseline from 8-bit grey, grey and alpha, RGB or RGBA (alpha dropped); quality 1–100 (Annex K tables scaled as IJG does); 4:4:4, 4:2:2, 4:2:0; a restart marker after every MCU row; optimized Huffman tables; an embedded ICC profile.

Not supported: arithmetic coding, lossless and hierarchical JPEG, 12-bit samples, heights defined by DNL.

## Files

| File | Responsibility |
| --- | --- |
| `mod.rs` | `info`, `decode`, `decode_rgba8`, `DecodeOptions`, `Info`; re-exports |
| `error.rs` | `Error`: `Corrupt { what, at }` (byte offset), `Unsupported`, `Invalid`, `TooLarge` |
| `headers.rs` | byte reader, markers, frame header, DQT, DHT, scan headers with progressive order checks |
| `huffman.rs` | decoding tables: 11-bit lookahead; a second table giving run, value and bits used for code plus magnitude together (end of block and zero runs too); the bit reader (64-bit, eight bytes a refill while none is 0xFF, the byte path out of line and by value so the state stays in registers) |
| `coefficients.rs` | coefficient layouts: MCU order for one-scan baseline frames (any run of MCUs is one slice a task owns), planes per component otherwise, with each block's nonzero mask for progressive frames |
| `scan.rs` | entropy decoding: baseline blocks, DC first/refine, AC first/refine (refinement walks the nonzero mask, never every position), restart markers |
| `idct.rs` | exact float IDCT (even/odd halves over eight lanes); reduced 1/2–1/8 IDCTs as means of the full-size points |
| `color.rs` | bilinear upsampling at pixel centres, JFIF/Adobe color conversion |
| `render.rs` | the plan (IDCT size, planes, sampling ratios per component) and its run: every band's IDCT, then every band's colors, each on the workers |
| `decode.rs` | the marker walk and the three routes: restart intervals in parallel; a one-scan baseline frame decoded MCU row by MCU row; whole-plane scans |
| `icc.rs` | APP2 ICC profile joining with libjpeg-turbo's rules |
| `encode/mod.rs` | encoder API, frame, quantization tables (Annex K scaled as IJG), headers, rows on the workers joined in order |
| `encode/transform.rs` | RGB → YCbCr, edge repetition, chroma averaging, exact float forward DCT, quantization |
| `encode/entropy.rs` | Annex K Huffman tables, optimal tables (K.2), bit writer, the joiner (word-wide bit shifting, byte stuffing) |

A color band reads plane rows of the bands either side (vertical interpolation), so every band's IDCT runs before any band's colors. Encoding: each MCU row is converted, transformed and Huffman-coded into its own bit stream, starting from the DC predictors of the row before (it transforms that row's last MCU again rather than wait); the calling thread joins the streams in order. How many workers run never changes the pixels or the bytes.

Accuracy: the IDCT is the exact transform to float rounding, upsampling keeps fractions (no rounding between upsampling and color conversion), and color conversion rounds once. Against libjpeg-turbo (islow, fancy upsampling) every fixture is within 4 (mean under 0.4), except 4:1 and 3:2 chroma, which libjpeg replicates and this decoder interpolates.

## Tests and oracles

- `tests/decode.rs`: 64 fixtures (synthetic pictures encoded by Pillow, libjpeg-turbo's cjpeg and TurboJPEG: every sampling, baseline and progressive, grey, RGB, CMYK, YCCK, restarts) decode within 4 of libjpeg-turbo's decodes (`tests/fixtures/reference`, written by `tools/jpeg_oracle.py references`), full size and 1/2, 1/4, 1/8; worker counts and routes agree byte for byte; every truncation of five files fails as `Corrupt`; 2 400 mutated files never panic; the ICC files join as libjpeg-turbo joined them (`tests/fixtures/icc/expected.txt`).
- `tests/encode.rs`: sizes 1×1 to 300×200, 1–4 channels, six quality and subsampling cases, restart and optimize on and off decode above PSNR floors; worker counts never change the bytes; optimized tables are smaller; ICC profiles over 64 KB embed and read back.
- `tools/jpeg_oracle.py check DIR` decodes the encoder tests' files with libjpeg-turbo (`KINOGAKI_JPEG_ORACLE_DIR=DIR cargo test --test encode`) and checks their PSNR against the source.

The oracles run locally (Pillow, so libjpeg-turbo) and are never dependencies.
