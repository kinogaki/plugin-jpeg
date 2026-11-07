//! The decoder: the walk over marker segments, each scan's entropy
//! decoding, and the render.
//!
//! Three routes, all giving the same pixels, the bands of every one
//! rendered in parallel on the process's workers:
//! - A baseline frame in one scan with restart intervals: the intervals
//!   entropy-decode in parallel too (each owns its run of MCUs in the MCU
//!   layout).
//! - Any other baseline frame in one scan: the calling thread
//!   entropy-decodes it MCU row by MCU row.
//! - Progressive frames and baseline frames of several scans: every scan
//!   decodes into whole planes of coefficients.
use crate::jpeg::coefficients::{Blocks, McuShape, Store};
use crate::jpeg::color::Model;
use crate::jpeg::headers::{Frame, Quantization, Reader, Scan, marker, parse_huffman};
use crate::jpeg::huffman::{Bits, Table, next_marker};
use crate::jpeg::parallel::map;
use crate::jpeg::render::{self, Plan};
use crate::jpeg::scan::{Cursor, Kind, ScanContext, Span, decode_mcus};
use crate::jpeg::{DecodeOptions, Error};
use kinogaki_studio_api::format::ReadBudget;
use std::sync::Mutex;

/// A decoded picture: 8-bit samples, `step` a pixel.
pub(crate) struct Decoded {
    pub width: usize,
    pub height: usize,
    pub step: usize,
    pub pixels: Vec<u8>,
}

/// Everything read from the marker segments so far.
struct State {
    frame: Option<Frame>,
    quant: Quantization,
    tables: [Option<Box<Table>>; 8],
    restart: usize,
    adobe: Option<u8>,
    /// Each component's coefficient coding so far (progressive order
    /// checks; a baseline component's single scan).
    progress: Vec<[i8; 64]>,
    store: Option<Store>,
    /// A one-scan baseline picture renders during its scan.
    rendered: Option<Vec<u8>>,
}

/// Decodes `data` at `options.scale` into `step`-byte pixels (`step` 0 is
/// the picture's own: 1 grey, 3 color).
pub(crate) fn decode(data: &[u8], options: &DecodeOptions, rgba: bool) -> Result<Decoded, Error> {
    let scale = usize::from(options.scale);
    if !matches!(scale, 1 | 2 | 4 | 8) {
        return Err(Error::Invalid("scale must be 1, 2, 4 or 8"));
    }
    if data.get(..2) != Some(&[0xFF, marker::SOI]) {
        return Err(Error::corrupt("not a JPEG file", 0));
    }
    let mut s = State {
        frame: None,
        quant: Quantization::default(),
        tables: Default::default(),
        restart: 0,
        adobe: None,
        progress: Vec::new(),
        store: None,
        rendered: None,
    };
    let mut r = Reader::new(data, 2);
    let budget = &options.budget;
    let stop = budget.cancel().shared_flag();
    let step_of = |frame: &Frame| {
        if rgba {
            4
        } else if frame.components.len() == 1 {
            1
        } else {
            3
        }
    };
    loop {
        if r.pos >= data.len() {
            return Err(Error::corrupt("end of image marker missing", r.pos));
        }
        let at = r.pos;
        let code = r.marker()?;
        match code {
            marker::EOI => break,
            marker::RST0..=marker::RST7 => continue,
            marker::SOF0 | marker::SOF1 | marker::SOF2 => {
                let (payload, start) = r.segment()?;
                if s.frame.is_some() {
                    return Err(Error::corrupt("second frame header", at));
                }
                let frame = Frame::parse(payload, start, code)?;
                if frame.width as u64 * frame.height as u64 > options.max_pixels {
                    return Err(Error::TooLarge);
                }
                let (width, height) = (frame.width.div_ceil(scale), frame.height.div_ceil(scale));
                budget.allocate((width * height * step_of(&frame)) as u64)?;
                s.progress = vec![[-1; 64]; frame.components.len()];
                s.frame = Some(frame);
            }
            code if marker::unsupported_frame(code) => {
                return Err(Error::Unsupported(
                    "lossless, hierarchical or arithmetic coding",
                ));
            }
            marker::DQT => {
                let (payload, start) = r.segment()?;
                s.quant.parse(payload, start)?;
            }
            marker::DHT => {
                let (payload, start) = r.segment()?;
                parse_huffman(payload, start, &mut s.tables)?;
            }
            marker::DRI => {
                let (payload, start) = r.segment()?;
                if payload.len() != 2 {
                    return Err(Error::corrupt(
                        "restart interval segment is not two bytes",
                        start,
                    ));
                }
                s.restart = usize::from(u16::from_be_bytes([payload[0], payload[1]]));
            }
            marker::APP14 => {
                let (payload, _) = r.segment()?;
                if payload.len() >= 12 && payload.starts_with(b"Adobe") {
                    s.adobe = Some(payload[11]);
                }
            }
            marker::SOS => {
                let (payload, start) = r.segment()?;
                let frame = s
                    .frame
                    .as_ref()
                    .ok_or(Error::corrupt("scan before the frame header", at))?;
                let scan = Scan::parse(payload, start, frame, &mut s.progress)?;
                let context = ScanContext::new(frame, &scan, (&s.tables, &stop), s.restart, start)?;
                if s.store.is_none() && s.rendered.is_none() {
                    coefficients(frame, data.len(), budget)?;
                }
                let entropy = r.pos;
                let whole = scan.components.len() == frame.components.len();
                r.pos = if !frame.progressive && whole && s.store.is_none() && s.rendered.is_none()
                {
                    check_quant(frame, &s.quant, at)?;
                    let plan = Plan::new(
                        frame,
                        &s.quant,
                        Model::of(&ids(frame), s.adobe),
                        scale,
                        step_of(frame),
                    );
                    let (pixels, end) = one_scan(frame, &context, data, entropy, &plan)?;
                    s.rendered = Some(pixels);
                    end
                } else {
                    if s.rendered.is_some() {
                        return Err(Error::corrupt("scan after the frame's only scan", at));
                    }
                    let store = s.store.get_or_insert_with(|| Store::planar(frame));
                    whole_scan(frame, &context, data, entropy, store)?
                };
            }
            _ => {
                r.segment()?;
            }
        }
    }
    let frame = s
        .frame
        .ok_or(Error::corrupt("no frame header", data.len()))?;
    let step = step_of(&frame);
    let (width, height) = (frame.width.div_ceil(scale), frame.height.div_ceil(scale));
    if let Some(pixels) = s.rendered {
        return Ok(Decoded {
            width,
            height,
            step,
            pixels,
        });
    }
    let store = s.store.ok_or(Error::corrupt("no scan", data.len()))?;
    check_quant(&frame, &s.quant, data.len())?;
    if s.progress.iter().any(|p| p[0] == -1) {
        return Err(Error::corrupt("a component has no DC scan", data.len()));
    }
    let plan = Plan::new(
        &frame,
        &s.quant,
        Model::of(&ids(&frame), s.adobe),
        scale,
        step,
    );
    let bands = (0..frame.mcuy)
        .map(|band| store.band(&frame, band))
        .collect();
    let pixels = render::render_all(&plan, bands);
    Ok(Decoded {
        width,
        height,
        step,
        pixels,
    })
}

/// Checks the coefficients `frame` keeps against `budget` before they are
/// allocated, `input` the file's length. Every block a file codes takes at
/// least a bit (its DC difference's code), so a file of `input` bytes codes
/// at most 8 blocks a byte, 1,024 bytes of coefficients: a file claiming a
/// frame larger than that holds can't be one.
fn coefficients(frame: &Frame, input: usize, budget: &ReadBudget) -> Result<(), Error> {
    let blocks = |c: &crate::jpeg::headers::Component| (c.actual_bx * c.actual_by) as u64;
    let coded: u64 = frame.components.iter().map(blocks).sum();
    budget.expand(input as u64, coded * 128)?;
    let mask = if frame.progressive { 8 } else { 0 };
    let kept = (frame.components.iter()).map(|c| (c.bx * c.by * (128 + mask)) as u64);
    budget.allocate(kept.sum())?;
    Ok(())
}

fn ids(frame: &Frame) -> Vec<u8> {
    frame.components.iter().map(|c| c.id).collect()
}

fn check_quant(frame: &Frame, quant: &Quantization, at: usize) -> Result<(), Error> {
    if frame.components.iter().any(|c| !quant.present[c.quant]) {
        return Err(Error::corrupt(
            "a component's quantization table is missing",
            at,
        ));
    }
    Ok(())
}

/// Decodes a scan into whole planes of coefficients; where the scan's
/// data ends.
fn whole_scan(
    frame: &Frame,
    context: &ScanContext,
    data: &[u8],
    entropy: usize,
    store: &mut Store,
) -> Result<usize, Error> {
    let Store::Planar { planes, masks } = store else {
        unreachable!("several scans use the planar layout");
    };
    let mut blocks = Blocks::Planar {
        planes: planes.iter_mut().map(|p| p.as_mut_slice()).collect(),
        masks: masks.iter_mut().map(|m| m.as_mut_slice()).collect(),
        bx: std::array::from_fn(|i| frame.components.get(i).map_or(0, |c| c.bx)),
        first_rows: [0; 4],
    };
    let mut bits = Bits::new(data, entropy);
    let mut cursor = Cursor::at_interval(0);
    let kind = Kind::of(frame.progressive, context);
    decode_mcus(
        kind,
        context,
        &mut bits,
        &mut cursor,
        &mut blocks,
        Span {
            mcus: 0..context.mcus(),
            started: true,
        },
    )?;
    Ok(next_marker(data, bits.pos, true))
}

/// Decodes and renders a baseline frame coded in one scan; the pixels and
/// where the scan's data ends.
fn one_scan(
    frame: &Frame,
    context: &ScanContext,
    data: &[u8],
    entropy: usize,
    plan: &Plan,
) -> Result<(Vec<u8>, usize), Error> {
    let interleaved = context.interleaved();
    let shape = McuShape::of(frame);
    let mut coefficients = match interleaved {
        true => Store::mcu(frame),
        false => Store::planar(frame),
    };
    let mut end = None;
    if let (Store::Mcu(all), true) = (&mut coefficients, context.restart > 0) {
        end = parallel_intervals(context, data, entropy, all, shape)?;
    }
    let end = match end {
        Some(end) => end,
        None => sequential(frame, context, data, entropy, &mut coefficients, shape)?,
    };
    let bands = (0..frame.mcuy)
        .map(|band| coefficients.band(frame, band))
        .collect();
    Ok((render::render_all(plan, bands), end))
}

/// Entropy-decodes a one-scan frame MCU row by MCU row into
/// `coefficients`; where the scan's data ends.
fn sequential(
    frame: &Frame,
    context: &ScanContext,
    data: &[u8],
    entropy: usize,
    coefficients: &mut Store,
    shape: McuShape,
) -> Result<usize, Error> {
    let chunks: Vec<&mut [i16]> = match coefficients {
        Store::Mcu(all) => all.chunks_mut(frame.mcux * shape.len()).collect(),
        Store::Planar { planes, .. } => {
            let c = &frame.components[0];
            planes[0].chunks_mut(c.bx * c.v * 64).collect()
        }
    };
    let mut bits = Bits::new(data, entropy);
    let mut cursor = Cursor::at_interval(0);
    for (band, chunk) in chunks.into_iter().enumerate() {
        let (mut blocks, mcus) = if context.interleaved() {
            let (first, end) = (band * frame.mcux, (band + 1) * frame.mcux);
            let blocks = Blocks::Mcu {
                data: chunk,
                shape,
                first,
            };
            (blocks, first..end)
        } else {
            // One component: the band is `v` block rows of the scan's.
            let c = &frame.components[0];
            let rows = (band * c.v).min(c.actual_by)..((band + 1) * c.v).min(c.actual_by);
            let blocks = Blocks::Planar {
                planes: vec![chunk],
                masks: Vec::new(),
                bx: [c.bx, 0, 0, 0],
                first_rows: [band * c.v, 0, 0, 0],
            };
            (blocks, rows.start * context.mcux..rows.end * context.mcux)
        };
        let span = Span {
            mcus,
            started: band == 0,
        };
        decode_mcus(
            Kind::Sequential,
            context,
            &mut bits,
            &mut cursor,
            &mut blocks,
            span,
        )?;
    }
    Ok(next_marker(data, bits.pos, true))
}

/// Entropy-decodes the restart intervals of a one-scan interleaved frame
/// on the process's workers into `all` (MCU layout); where the scan ends,
/// or `None` when its restart markers are not all there (the sequential
/// route then finds what is wrong).
fn parallel_intervals(
    context: &ScanContext,
    data: &[u8],
    entropy: usize,
    all: &mut [i16],
    shape: McuShape,
) -> Result<Option<usize>, Error> {
    let total = context.mcus();
    let interval = context.restart;
    let count = total.div_ceil(interval);
    // Where each interval's data starts: after the previous one's marker.
    let mut starts = Vec::with_capacity(count);
    starts.push(entropy);
    let mut pos = entropy;
    while starts.len() < count {
        let at = next_marker(data, pos, false);
        match data.get(at + 1) {
            Some(code) if (marker::RST0..=marker::RST7).contains(code) => {
                pos = at + 2;
                starts.push(pos);
            }
            _ => return Ok(None),
        }
    }
    let chunks: Vec<Mutex<&mut [i16]>> = all
        .chunks_mut(interval * shape.len())
        .map(Mutex::new)
        .collect();
    // Each interval's result, the last one's where the scan ends; the first
    // failure by interval wins.
    let decoded = map(count, |i| {
        let mut chunk = chunks[i].lock().unwrap_or_else(|e| e.into_inner());
        let first = i * interval;
        let last = (first + interval).min(total);
        let mut bits = Bits::new(data, starts[i]);
        let mut cursor = Cursor::at_interval(i);
        let mut blocks = Blocks::Mcu {
            data: &mut chunk,
            shape,
            first,
        };
        let span = Span {
            mcus: first..last,
            started: true,
        };
        decode_mcus(
            Kind::Sequential,
            context,
            &mut bits,
            &mut cursor,
            &mut blocks,
            span,
        )?;
        if i + 1 < count {
            bits.restart(cursor.expected)
                .map_err(|what| Error::corrupt(what, bits.pos))?;
            return Ok(0);
        }
        Ok(next_marker(data, bits.pos, true))
    });
    let ends = decoded.into_iter().collect::<Result<Vec<usize>, Error>>()?;
    Ok(ends.last().copied())
}

/// The frame header of `data`, read without decoding anything else.
pub(crate) fn frame_of(data: &[u8]) -> Result<Frame, Error> {
    if data.get(..2) != Some(&[0xFF, marker::SOI]) {
        return Err(Error::corrupt("not a JPEG file", 0));
    }
    let mut r = Reader::new(data, 2);
    loop {
        let at = r.pos;
        let code = r.marker()?;
        match code {
            marker::SOF0 | marker::SOF1 | marker::SOF2 => {
                let (payload, start) = r.segment()?;
                return Frame::parse(payload, start, code);
            }
            marker::EOI | marker::SOS => {
                return Err(Error::corrupt("no frame header before the scan", at));
            }
            marker::RST0..=marker::RST7 => {}
            code if marker::unsupported_frame(code) => {
                return Err(Error::Unsupported(
                    "lossless, hierarchical or arithmetic coding",
                ));
            }
            _ => {
                r.segment()?;
            }
        }
    }
}
