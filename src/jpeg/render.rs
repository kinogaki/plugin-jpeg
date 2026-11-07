//! Rendering coefficients into pixels: the plan (each component's IDCT
//! size, plane and sampling ratio against output pixels) and its run on
//! the process's workers.
//!
//! Work goes in MCU-row bands. Every band's IDCT turns its coefficients
//! into its rows of each component plane; then every band's colors
//! upsample and convert its output rows, reading plane rows of the bands
//! either side too.
use crate::jpeg::coefficients::BandView;
use crate::jpeg::color::{self, Axis, Model};
use crate::jpeg::headers::{Frame, Quantization};
use crate::jpeg::idct::{Scaled, idct8};
use kinogaki_studio_api::geometry::parallel::map;
use std::sync::Mutex;

/// One component's part of the plan.
pub(crate) struct ComponentPlan {
    pub v: usize,
    pub bx: usize,
    /// IDCT output block size: 8 full size, fewer when DCT-scaled.
    pub size: usize,
    /// Plane samples across (`bx * size`) and rows a band holds.
    pub stride: usize,
    pub band_rows: usize,
    pub quant: [f32; 64],
    pub x: Axis,
    pub y: Axis,
    /// Plane samples across that output pixels read.
    pub reach: usize,
}

/// How a frame renders at a scale into 8-bit pixels.
pub(crate) struct Plan {
    pub width: usize,
    pub height: usize,
    /// Bytes a pixel: 1, 3 or 4.
    pub step: usize,
    pub model: Model,
    pub components: Vec<ComponentPlan>,
    /// Output rows an MCU row gives.
    pub band_rows: usize,
    scaled: Scaled,
}

impl Plan {
    /// The plan for `frame` at 1/`scale` with `step` bytes a pixel.
    pub fn new(
        frame: &Frame,
        quant: &Quantization,
        model: Model,
        scale: usize,
        step: usize,
    ) -> Plan {
        let n = 8 / scale;
        let width = frame.width.div_ceil(scale);
        let height = frame.height.div_ceil(scale);
        let components = frame
            .components
            .iter()
            .map(|c| {
                // Scaled, a subsampled component takes a larger IDCT where
                // that replaces upsampling by a power of two both ways, as
                // libjpeg does.
                let mut size = n;
                while size < 8
                    && (frame.max_h * n).is_multiple_of(c.h * size * 2)
                    && (frame.max_v * n).is_multiple_of(c.v * size * 2)
                {
                    size *= 2;
                }
                let stride = c.bx * size;
                let rows = c.by * size;
                let (num_x, den_x) = (c.h * size, n * frame.max_h);
                let (num_y, den_y) = (c.v * size, n * frame.max_v);
                let valid_w = (width * num_x).div_ceil(den_x);
                let valid_h = (height * num_y).div_ceil(den_y);
                let halves = (num_x == den_x || num_x * 2 == den_x)
                    && (num_y == den_y || num_y * 2 == den_y);
                let replicate = scale > 1
                    && (num_x != den_x || num_y != den_y)
                    && (scale == 8 || !halves || valid_w <= 2);
                let x = Axis::new(width, num_x, den_x, valid_w, stride, replicate);
                let y = Axis::new(height, num_y, den_y, valid_h, rows, replicate);
                let reach = x.reach(width, stride);
                let table = &quant.tables[c.quant];
                ComponentPlan {
                    v: c.v,
                    bx: c.bx,
                    size,
                    stride,
                    band_rows: c.v * size,
                    quant: std::array::from_fn(|i| f32::from(table[i])),
                    x,
                    y,
                    reach,
                }
            })
            .collect();
        Plan {
            width,
            height,
            step,
            model,
            components,
            band_rows: frame.max_v * n,
            scaled: Scaled::new(),
        }
    }

    /// Output bytes a row.
    pub fn row_bytes(&self) -> usize {
        self.width * self.step
    }

    /// The IDCT of band `band`'s coefficients into each component's band
    /// of plane rows.
    fn idct(&self, coefficients: &BandView) -> Vec<Vec<u8>> {
        self.components
            .iter()
            .enumerate()
            .map(|(c, plan)| {
                let mut plane = vec![0u8; plan.stride * plan.band_rows];
                // Columns past what output pixels read are left zero.
                let columns = plan.reach.div_ceil(plan.size).min(plan.bx);
                for row in 0..plan.v {
                    let rows = &mut plane
                        [row * plan.size * plan.stride..(row + 1) * plan.size * plan.stride];
                    for column in 0..columns {
                        let block = coefficients.block(c, row, column, plan.bx);
                        let out = &mut rows[column * plan.size..];
                        if plan.size == 8 {
                            idct8(block, &plan.quant, out, plan.stride);
                        } else {
                            self.scaled
                                .idct(plan.size, block, &plan.quant, out, plan.stride);
                        }
                    }
                }
                plane
            })
            .collect()
    }

    /// Upsamples and converts band `band`'s output rows into `out`.
    fn color(&self, band: usize, planes: &[Vec<Vec<u8>>], out: &mut [u8], scratch: &mut Scratch) {
        let first = band * self.band_rows;
        let rows = out.len() / self.row_bytes();
        scratch.prepare(self);
        for (local, line) in out
            .chunks_exact_mut(self.row_bytes())
            .enumerate()
            .take(rows)
        {
            let y = first + local;
            for (c, plan) in self.components.iter().enumerate() {
                let plane_row = |r: usize| -> &[u8] {
                    let band = &planes[r / plan.band_rows];
                    let at = (r % plan.band_rows) * plan.stride;
                    &band[c][at..at + plan.stride]
                };
                let reach = plan.reach;
                let Scratch { rows, vertical } = &mut *scratch;
                // The plane row (or vertical blend) this output row reads.
                let source: &[f32] = match &plan.y {
                    Axis::Identity => {
                        widen(&plane_row(y)[..reach], &mut vertical[..reach]);
                        &vertical[..reach]
                    }
                    Axis::Replicate { num, den } => {
                        widen(&plane_row(y * num / den)[..reach], &mut vertical[..reach]);
                        &vertical[..reach]
                    }
                    Axis::Linear {
                        first,
                        second,
                        weight,
                    } => {
                        let (a, b, w) = (
                            plane_row(first[y] as usize),
                            plane_row(second[y] as usize),
                            weight[y],
                        );
                        if w == 0.0 {
                            widen(&a[..reach], &mut vertical[..reach]);
                        } else {
                            for ((v, &a), &b) in vertical[..reach]
                                .iter_mut()
                                .zip(&a[..reach])
                                .zip(&b[..reach])
                            {
                                *v = f32::from(a) * (1.0 - w) + f32::from(b) * w;
                            }
                        }
                        &vertical[..reach]
                    }
                };
                let row = &mut rows[c][..self.width];
                match &plan.x {
                    Axis::Identity => row.copy_from_slice(&source[..self.width]),
                    Axis::Replicate { num, den } => {
                        for (x, v) in row.iter_mut().enumerate() {
                            *v = source[x * num / den];
                        }
                    }
                    Axis::Linear {
                        first,
                        second,
                        weight,
                    } => {
                        for (((v, &a), &b), &w) in row.iter_mut().zip(first).zip(second).zip(weight)
                        {
                            *v = source[a as usize] * (1.0 - w) + source[b as usize] * w;
                        }
                    }
                }
            }
            color::convert(self.model, &scratch.rows, self.width, line, self.step);
        }
    }
}

#[inline]
fn widen(bytes: &[u8], out: &mut [f32]) {
    for (o, &b) in out.iter_mut().zip(bytes) {
        *o = f32::from(b);
    }
}

/// A worker's rows: each component's upsampled output row and a
/// vertically blended plane row.
#[derive(Default)]
struct Scratch {
    rows: Vec<Vec<f32>>,
    vertical: Vec<f32>,
}

impl Scratch {
    fn prepare(&mut self, plan: &Plan) {
        if self.rows.len() != plan.components.len() {
            self.rows = vec![vec![0.0; plan.width]; plan.components.len()];
            let widest = plan.components.iter().map(|c| c.stride).max().unwrap_or(0);
            self.vertical = vec![0.0; widest.max(plan.width)];
        }
    }
}

/// Renders a frame whose coefficients are all decoded, on the process's
/// workers: every band's IDCT, then every band's colors (which read their
/// neighbours' planes).
pub(crate) fn render_all(plan: &Plan, bands: Vec<BandView>) -> Vec<u8> {
    let planes = map(bands.len(), |band| plan.idct(&bands[band]));
    let mut out = vec![0u8; plan.height * plan.row_bytes()];
    let rows: Vec<&mut [u8]> = out.chunks_mut(plan.band_rows * plan.row_bytes()).collect();
    let rows: Vec<Mutex<&mut [u8]>> = rows.into_iter().map(Mutex::new).collect();
    map(rows.len(), |band| {
        let mut rows = rows[band].lock().unwrap_or_else(|e| e.into_inner());
        plan.color(band, &planes, &mut rows, &mut Scratch::default());
    });
    out
}
