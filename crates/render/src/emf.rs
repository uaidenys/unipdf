//! Vector page output for the Windows spooler: one page is interpreted with hayro and written as
//! an enhanced metafile (EMF). The spooler plays the metafile as drawing commands, so paths, text
//! outlines, strokes and clips stay vectors on the printer's side, as with Adobe's print path.
//!
//! The metafile uses logical units of 0.01 mm with y pointing up (`MM_HIMETRIC`). Anything this
//! writer cannot reproduce faithfully makes the page [`EmfError::Unsupported`], so the caller can
//! use its own path for the job: shadings and patterns, soft masks, blend modes, transparency
//! groups that paint, dashed strokes, rotated or skewed images, stencil masks and Type 3 glyphs.

use std::collections::HashMap;
use std::sync::Arc;

use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, FillRule, GlyphDrawMode, Image, ImageData, InterpreterCache, InterpreterSettings, LumaData, Paint,
    PathDrawMode, SoftMask, TransformExt, interpret_page,
};
use hayro::hayro_syntax::Pdf;
use kurbo::{Affine, BezPath, Cap, Join, PathEl, Point, Rect};

/// Logical units per PDF point: 0.01 mm is 2540 / 72 of a point.
const UNITS_PER_POINT: f64 = 2540.0 / 72.0;

/// Largest image (in pixels) written as-is; bigger ones are left to the caller's path.
const MAX_IMAGE_PIXELS: u64 = 40_000_000;

/// A page drawn as an enhanced metafile, with its size in points.
#[derive(Debug, Clone, PartialEq)]
pub struct EmfPage {
    /// The complete metafile.
    pub bytes: Vec<u8>,
    /// Page width in points.
    pub width_pt: f64,
    /// Page height in points.
    pub height_pt: f64,
}

/// Why a page could not be written as a metafile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmfError {
    /// The page uses something this writer does not reproduce; the caller falls back.
    Unsupported(&'static str),
    /// The document could not be read.
    Invalid(String),
}

impl std::fmt::Display for EmfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(what) => write!(f, "the page uses {what}, which the metafile path does not draw"),
            Self::Invalid(e) => write!(f, "the document could not be read: {e}"),
        }
    }
}

/// The number of pages in the PDF in `bytes`.
pub fn page_count(bytes: &[u8]) -> Result<usize, EmfError> {
    let pdf = Pdf::new(Arc::new(bytes.to_vec())).map_err(|e| EmfError::Invalid(format!("{e:?}")))?;
    Ok(pdf.pages().len())
}

/// Writes page `page_index` (0-based) of the PDF in `bytes` as a metafile.
pub fn page_to_emf(bytes: &[u8], page_index: usize) -> Result<EmfPage, EmfError> {
    let pdf = Pdf::new(Arc::new(bytes.to_vec())).map_err(|e| EmfError::Invalid(format!("{e:?}")))?;
    let page = pdf.pages().get(page_index).ok_or_else(|| EmfError::Invalid(format!("there is no page {}", page_index + 1)))?;
    let (width, height) = page.render_dimensions();
    let (width, height) = (f64::from(width), f64::from(height));
    let cache = InterpreterCache::new();
    let initial = page.initial_transform(true).to_kurbo();
    let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, width, height), &cache, page.xref(), InterpreterSettings::default());
    let mut device = EmfDevice::new();
    interpret_page(page, &mut ctx, &mut device);
    if let Some(what) = device.unsupported {
        return Err(EmfError::Unsupported(what));
    }
    let frame = (units(width), units(height));
    Ok(EmfPage { bytes: device.finish(frame), width_pt: width, height_pt: height })
}

/// Points to logical units, rounded, clamped so a stray coordinate cannot overflow 32 bits.
fn units(points: f64) -> i32 {
    (points * UNITS_PER_POINT).round().clamp(-1.0e9, 1.0e9) as i32
}

// Record types (MS-EMF 2.3.1).
const EMR_EOF: u32 = 14;
const EMR_SETMAPMODE: u32 = 17;
const EMR_SETPOLYFILLMODE: u32 = 19;
const EMR_SAVEDC: u32 = 33;
const EMR_RESTOREDC: u32 = 34;
const EMR_SELECTOBJECT: u32 = 37;
const EMR_CREATEBRUSHINDIRECT: u32 = 39;
const EMR_MOVETOEX: u32 = 27;
const EMR_POLYBEZIERTO: u32 = 5;
const EMR_LINETO: u32 = 54;
const EMR_BEGINPATH: u32 = 59;
const EMR_ENDPATH: u32 = 60;
const EMR_CLOSEFIGURE: u32 = 61;
const EMR_FILLPATH: u32 = 62;
const EMR_STROKEPATH: u32 = 63;
const EMR_SELECTCLIPPATH: u32 = 67;
const EMR_STRETCHDIBITS: u32 = 81;
const EMR_EXTCREATEPEN: u32 = 95;

const MM_HIMETRIC: u32 = 3;
const RGN_AND: u32 = 1;
const ALTERNATE: u32 = 1;
const WINDING: u32 = 2;
const BI_RGB: u32 = 0;
const SRCCOPY: u32 = 0x00CC_0020;
const EMF_SIGNATURE: u32 = 0x464D_4520;
const EMF_VERSION: u32 = 0x0001_0000;

// Pen styles (wingdi.h).
const PS_GEOMETRIC: u32 = 0x0001_0000;
const PS_ENDCAP_ROUND: u32 = 0x0000_0000;
const PS_ENDCAP_SQUARE: u32 = 0x0000_0100;
const PS_ENDCAP_FLAT: u32 = 0x0000_0200;
const PS_JOIN_ROUND: u32 = 0x0000_0000;
const PS_JOIN_BEVEL: u32 = 0x0000_1000;
const PS_JOIN_MITER: u32 = 0x0000_2000;

/// The records of one metafile body, before the header and the end record are added.
#[derive(Default)]
struct Records {
    bytes: Vec<u8>,
    count: u32,
}

impl Records {
    fn record(&mut self, kind: u32, body: &[u8]) {
        self.bytes.extend_from_slice(&kind.to_le_bytes());
        self.bytes.extend_from_slice(&(8 + body.len() as u32).to_le_bytes());
        self.bytes.extend_from_slice(body);
        self.count += 1;
    }

    fn point(&mut self, kind: u32, x: i32, y: i32) {
        let mut body = Vec::with_capacity(8);
        body.extend_from_slice(&x.to_le_bytes());
        body.extend_from_slice(&y.to_le_bytes());
        self.record(kind, &body);
    }
}

fn le(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// `COLORREF`: 0x00BBGGRR.
fn colorref(rgb: [u8; 3]) -> u32 {
    u32::from(rgb[0]) | (u32::from(rgb[1]) << 8) | (u32::from(rgb[2]) << 16)
}

/// A frame of the interpreter's state that the device must undo when it is popped.
enum Frame {
    /// A clip: saved with `SAVEDC` and restored with `RESTOREDC`.
    Clip,
    /// A transparency group that paints exactly as its content does: nothing to undo.
    Group,
}

struct EmfDevice {
    records: Records,
    next_handle: u32,
    brushes: HashMap<u32, u32>,
    pens: HashMap<(u32, u32, u32, u32), u32>,
    frames: Vec<Frame>,
    unsupported: Option<&'static str>,
}

impl EmfDevice {
    fn new() -> Self {
        let mut records = Records::default();
        records.record(EMR_SETMAPMODE, &le(&[MM_HIMETRIC]));
        Self { records, next_handle: 1, brushes: HashMap::new(), pens: HashMap::new(), frames: Vec::new(), unsupported: None }
    }

    fn refuse(&mut self, what: &'static str) {
        self.unsupported.get_or_insert(what);
    }

    /// Device point (points, y down) to a logical point. `MM_HIMETRIC` has y up from the origin,
    /// which is the top-left corner of the play rectangle, so a point below the top is negative:
    /// the page then runs down from the origin as the device's rows do.
    fn logical(&self, p: Point) -> (i32, i32) {
        (units(p.x), -units(p.y))
    }

    fn take_handle(&mut self) -> u32 {
        let handle = self.next_handle;
        self.next_handle += 1;
        handle
    }

    fn brush(&mut self, rgb: [u8; 3]) -> u32 {
        let key = colorref(rgb);
        if let Some(&handle) = self.brushes.get(&key) {
            return handle;
        }
        let handle = self.take_handle();
        // LOGBRUSH: solid style, colour, unused hatch.
        let mut body = le(&[handle]);
        body.extend(le(&[0, key, 0]));
        self.records.record(EMR_CREATEBRUSHINDIRECT, &body);
        self.brushes.insert(key, handle);
        handle
    }

    fn pen(&mut self, width: i32, cap: Cap, join: Join, rgb: [u8; 3]) -> u32 {
        let color = colorref(rgb);
        let cap_style = match cap {
            Cap::Butt => PS_ENDCAP_FLAT,
            Cap::Round => PS_ENDCAP_ROUND,
            Cap::Square => PS_ENDCAP_SQUARE,
        };
        let join_style = match join {
            Join::Miter => PS_JOIN_MITER,
            Join::Round => PS_JOIN_ROUND,
            Join::Bevel => PS_JOIN_BEVEL,
        };
        let key = (width as u32, cap_style, join_style, color);
        if let Some(&handle) = self.pens.get(&key) {
            return handle;
        }
        let handle = self.take_handle();
        // EXTLOGPEN with no bitmap: a geometric pen of the given width in logical units.
        let mut body = le(&[handle, 0, 0, 0, 0]);
        body.extend(le(&[PS_GEOMETRIC | cap_style | join_style, width as u32, 0, color, 0, 0]));
        self.records.record(EMR_EXTCREATEPEN, &body);
        self.pens.insert(key, handle);
        handle
    }

    /// Records `path` (device points) as a path bracket, ready to fill or stroke or clip.
    fn path(&mut self, path: &BezPath) {
        self.records.record(EMR_BEGINPATH, &[]);
        let mut current = Point::ZERO;
        let mut start = Point::ZERO;
        for el in path.elements() {
            match *el {
                PathEl::MoveTo(p) => {
                    let (x, y) = self.logical(p);
                    self.records.point(EMR_MOVETOEX, x, y);
                    current = p;
                    start = p;
                }
                PathEl::LineTo(p) => {
                    let (x, y) = self.logical(p);
                    self.records.point(EMR_LINETO, x, y);
                    current = p;
                }
                PathEl::QuadTo(c, p) => {
                    // A quadratic curve is exact as a cubic with the control points lifted by 2/3.
                    let c1 = current + (c - current) * (2.0 / 3.0);
                    let c2 = p + (c - p) * (2.0 / 3.0);
                    self.cubic(c1, c2, p);
                    current = p;
                }
                PathEl::CurveTo(c1, c2, p) => {
                    self.cubic(c1, c2, p);
                    current = p;
                }
                PathEl::ClosePath => {
                    self.records.record(EMR_CLOSEFIGURE, &[]);
                    current = start;
                }
            }
        }
        self.records.record(EMR_ENDPATH, &[]);
    }

    fn cubic(&mut self, c1: Point, c2: Point, p: Point) {
        let mut body = le(&[0, 0, 0, 0, 3]); // RECTL bounds (unused by the player) and the count
        for point in [c1, c2, p] {
            let (x, y) = self.logical(point);
            body.extend_from_slice(&x.to_le_bytes());
            body.extend_from_slice(&y.to_le_bytes());
        }
        self.records.record(EMR_POLYBEZIERTO, &body);
    }

    fn fill(&mut self, path: &BezPath, rgb: [u8; 3], rule: FillRule) {
        self.path(path);
        let mode = fill_mode(rule);
        self.records.record(EMR_SETPOLYFILLMODE, &le(&[mode]));
        let brush = self.brush(rgb);
        self.records.record(EMR_SELECTOBJECT, &le(&[brush]));
        self.records.record(EMR_FILLPATH, &[0; 16]);
    }

    fn stroke(&mut self, path: &BezPath, rgb: [u8; 3], width: f64, cap: Cap, join: Join) {
        let width = units(width).max(1);
        let pen = self.pen(width, cap, join, rgb);
        self.path(path);
        self.records.record(EMR_SELECTOBJECT, &le(&[pen]));
        self.records.record(EMR_STROKEPATH, &[0; 16]);
    }

    /// The solid colour of `paint`, or `None` (with the page refused) for anything else.
    fn solid(&mut self, paint: &Paint<'_>) -> Option<[u8; 3]> {
        match paint {
            Paint::Color(c) => {
                let [r, g, b, a] = c.to_rgba().to_rgba8();
                if a != 255 {
                    self.refuse("a translucent colour");
                    return None;
                }
                Some([r, g, b])
            }
            Paint::Pattern(_) => {
                self.refuse("a pattern or a shading");
                None
            }
        }
    }

    fn draw_shape(&mut self, path: &BezPath, transform: Affine, paint: &Paint<'_>, mode: &PathDrawMode) {
        let Some(rgb) = self.solid(paint) else { return };
        let device = transform * path.clone();
        match mode {
            PathDrawMode::Fill(rule) => self.fill(&device, rgb, *rule),
            PathDrawMode::Stroke(props) => {
                if !props.dash_array.is_empty() {
                    self.refuse("a dashed stroke");
                    return;
                }
                // The width in points, scaled by the transform; `stroke` converts it to units.
                let scale = transform.determinant().abs().sqrt();
                self.stroke(&device, rgb, f64::from(props.line_width) * scale, props.line_cap, props.line_join);
            }
        }
    }

    /// An axis-aligned raster image, flipped to the orientation it lands in, as a 24-bit DIB.
    fn image(&mut self, data: ImageData, alpha: Option<LumaData>, transform: Affine) {
        let (sx, sy) = data.scale_factors();
        let (width, height) = (data.width(), data.height());
        if u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
            self.refuse("an image too large to place");
            return;
        }
        let m = transform * Affine::scale_non_uniform(f64::from(sx), f64::from(sy));
        let [a, b, c, d, e, f] = m.as_coeffs();
        if b.abs() > 1e-9 || c.abs() > 1e-9 {
            self.refuse("a rotated image");
            return;
        }
        let (w, h) = (f64::from(width), f64::from(height));
        let (x0, x1) = (e.min(e + a * w), e.max(e + a * w));
        let (y0, y1) = (f.min(f + d * h), f.max(f + d * h));
        // Pixel (0, 0) sits at the right edge when a < 0 and at the bottom edge when d < 0; the
        // DIB is written top to bottom and left to right, so those orders are reversed.
        let flip_x = a < 0.0;
        let flip_y = d < 0.0;
        let rgb = rgb_pixels(&data, alpha.as_ref());
        let bits = orient(&rgb, width as usize, height as usize, flip_x, flip_y);
        let stride = (width as usize * 3).next_multiple_of(4);
        let mut dib_bits = vec![0u8; stride * height as usize];
        for (row, src) in bits.chunks_exact(width as usize * 3).enumerate() {
            let out = &mut dib_bits[row * stride..row * stride + width as usize * 3];
            for (i, px) in src.chunks_exact(3).enumerate() {
                // DIB pixels are BGR.
                out[i * 3] = px[2];
                out[i * 3 + 1] = px[1];
                out[i * 3 + 2] = px[0];
            }
        }
        let (left, top) = (units(x0), -units(y0));
        let (dest_w, dest_h) = (units(x1) - units(x0), -(units(y1) - units(y0)));
        // BITMAPINFOHEADER with a negative height: rows run top to bottom.
        let mut bmi = le(&[40]);
        bmi.extend(le(&[width]));
        bmi.extend((-(height as i32)).to_le_bytes());
        bmi.extend(1u16.to_le_bytes());
        bmi.extend(24u16.to_le_bytes());
        bmi.extend(le(&[BI_RGB, dib_bits.len() as u32, 0, 0, 0, 0]));
        let mut body = vec![0u8; 16]; // bounds
        let fields = [left, top, 0, 0, width as i32, height as i32];
        body.extend(fields.iter().flat_map(|v| v.to_le_bytes()));
        // Offsets count from the start of the record, the 8-byte header included.
        body.extend(le(&[80, 40, 120, dib_bits.len() as u32, 0, SRCCOPY]));
        body.extend(dest_w.to_le_bytes());
        body.extend(dest_h.to_le_bytes());
        body.extend(bmi);
        body.extend(dib_bits);
        self.records.record(EMR_STRETCHDIBITS, &body);
    }

    /// The metafile: header, records and end record, with the sizes filled in.
    fn finish(self, frame: (i32, i32)) -> Vec<u8> {
        let mut records = self.records;
        // The end record: no palette, its offset, and the size of the record itself.
        records.record(EMR_EOF, &le(&[0, 16, 20]));
        let total = 88 + records.bytes.len() as u32;
        let (fw, fh) = frame;
        let mut out = Vec::with_capacity(total as usize);
        out.extend(le(&[EMR_HEADER_TYPE, 88]));
        for v in [0, 0, fw - 1, fh - 1, 0, 0, fw, fh] {
            out.extend(v.to_le_bytes());
        }
        out.extend(le(&[EMF_SIGNATURE, EMF_VERSION, total, records.count + 1]));
        out.extend((self.next_handle as u16).to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(le(&[0, 0, 0]));
        out.extend(le(&[1024, 768, 320, 240]));
        out.extend(records.bytes);
        out
    }
}

const EMR_HEADER_TYPE: u32 = 1;

/// The pixels as RGB over white, in the order they are stored.
fn rgb_pixels(data: &ImageData, alpha: Option<&LumaData>) -> Vec<u8> {
    let mut out = match data {
        ImageData::Rgb(d) => d.data.clone(),
        ImageData::Luma(d) => d.data.iter().flat_map(|&v| [v, v, v]).collect(),
    };
    if let Some(alpha) = alpha {
        for (px, &a) in out.chunks_exact_mut(3).zip(alpha.data.iter()) {
            for channel in px.iter_mut() {
                // Over white paper: c * a + 255 * (1 - a).
                *channel = ((u32::from(*channel) * u32::from(a) + 255 * (255 - u32::from(a)) + 127) / 255) as u8;
            }
        }
    }
    out
}

/// The image's rows and columns reversed where its orientation on the page is mirrored.
fn orient(rgb: &[u8], width: usize, height: usize, flip_x: bool, flip_y: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len());
    for row in 0..height {
        let src_row = if flip_y { height - 1 - row } else { row };
        let line = &rgb[src_row * width * 3..(src_row + 1) * width * 3];
        if flip_x {
            for px in line.chunks_exact(3).rev() {
                out.extend_from_slice(px);
            }
        } else {
            out.extend_from_slice(line);
        }
    }
    out
}

impl<'a> Device<'a> for EmfDevice {
    fn set_soft_mask(&mut self, mask: Option<SoftMask<'a>>) {
        if mask.is_some() {
            self.refuse("a soft mask");
        }
    }

    fn set_blend_mode(&mut self, blend_mode: BlendMode) {
        if blend_mode != BlendMode::Normal {
            self.refuse("a blend mode");
        }
    }

    fn draw_path(&mut self, path: &BezPath, transform: Affine, paint: &Paint<'a>, mode: &PathDrawMode) {
        self.draw_shape(path, transform, paint, mode);
    }

    fn push_clip_path(&mut self, clip: &ClipPath) {
        // The interpreter hands clip paths over in device space.
        self.records.record(EMR_SAVEDC, &[]);
        self.path(&clip.path);
        self.records.record(EMR_SETPOLYFILLMODE, &le(&[fill_mode(clip.fill)]));
        self.records.record(EMR_SELECTCLIPPATH, &le(&[RGN_AND]));
        self.frames.push(Frame::Clip);
    }

    fn push_transparency_group(&mut self, opacity: f32, mask: Option<SoftMask<'a>>, blend_mode: BlendMode) {
        if opacity == 1.0 && mask.is_none() && blend_mode == BlendMode::Normal {
            self.frames.push(Frame::Group);
        } else {
            self.refuse("a transparency group");
            self.frames.push(Frame::Group);
        }
    }

    fn draw_glyph(&mut self, glyph: &Glyph<'a>, transform: Affine, glyph_transform: Affine, paint: &Paint<'a>, mode: &GlyphDrawMode) {
        match glyph {
            Glyph::Outline(outline) => match mode {
                GlyphDrawMode::Fill => {
                    let path = outline.outline();
                    self.draw_shape(&path, transform * glyph_transform, paint, &PathDrawMode::Fill(FillRule::NonZero));
                }
                GlyphDrawMode::Stroke(props) => {
                    let path = outline.outline();
                    self.draw_shape(&path, transform * glyph_transform, paint, &PathDrawMode::Stroke(props.clone()));
                }
                GlyphDrawMode::Clip | GlyphDrawMode::Invisible => {}
            },
            Glyph::Type3(_) => self.refuse("a Type 3 font"),
        }
    }

    fn draw_image(&mut self, image: Image<'a, '_>, transform: Affine) {
        match image {
            Image::Raster(raster) => {
                // The closure runs once with the decoded pixels.
                let mut placed = None;
                raster.with_rgba(|data, alpha| placed = Some((data, alpha)), None);
                if let Some((data, alpha)) = placed {
                    self.image(data, alpha, transform);
                }
            }
            Image::Stencil(_) => self.refuse("a stencil mask"),
        }
    }

    fn pop_clip_path(&mut self) {
        if let Some(Frame::Clip) = self.frames.pop() {
            self.records.record(EMR_RESTOREDC, &le(&[(-1i32) as u32]));
        }
    }

    fn pop_transparency_group(&mut self) {
        self.frames.pop();
    }
}

/// `SETPOLYFILLMODE` value for a fill rule.
fn fill_mode(rule: FillRule) -> u32 {
    match rule {
        FillRule::EvenOdd => ALTERNATE,
        FillRule::NonZero => WINDING,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page PDF with `content` and `resources` on a 200 x 200 point page. Hayro reads it
    /// without a cross-reference table, as the raster tests' documents are read.
    fn pdf(content: &str, resources: &str) -> Vec<u8> {
        format!(
            "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << {resources} >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
            content.len()
        )
        .into_bytes()
    }

    /// The record types of a metafile, in order, after the header.
    fn record_types(emf: &[u8]) -> Vec<u32> {
        let mut types = Vec::new();
        let mut at = 88;
        while at + 8 <= emf.len() {
            let kind = u32::from_le_bytes(emf[at..at + 4].try_into().unwrap());
            let size = u32::from_le_bytes(emf[at + 4..at + 8].try_into().unwrap()) as usize;
            types.push(kind);
            if size < 8 {
                break;
            }
            at += size;
        }
        types
    }

    #[test]
    fn a_metafile_has_the_emf_header_and_ends_with_its_end_record() {
        let page = page_to_emf(&pdf("0 0 1 rg 10 10 50 50 re f", ""), 0).expect("draws");
        assert_eq!(u32::from_le_bytes(page.bytes[40..44].try_into().unwrap()), 0x464D_4520, "EMF signature");
        assert_eq!(u32::from_le_bytes(page.bytes[48..52].try_into().unwrap()) as usize, page.bytes.len(), "byte count");
        let types = record_types(&page.bytes);
        assert_eq!(types.first(), Some(&EMR_SETMAPMODE));
        assert_eq!(types.last(), Some(&EMR_EOF));
        assert!((page.width_pt - 200.0).abs() < 1e-6 && (page.height_pt - 200.0).abs() < 1e-6);
    }

    #[test]
    fn a_filled_rectangle_is_a_path_filled_with_its_colour() {
        let page = page_to_emf(&pdf("0 0 1 rg 10 10 50 50 re f", ""), 0).expect("draws");
        let types = record_types(&page.bytes);
        assert!(types.contains(&EMR_BEGINPATH) && types.contains(&EMR_FILLPATH), "a path bracket is filled: {types:?}");
        // COLORREF is 0x00BBGGRR: pure blue is 0x00FF0000.
        let blue = 0x00FF_0000u32.to_le_bytes();
        assert!(page.bytes.windows(4).any(|w| w == blue), "the brush holds pure blue");
    }

    #[test]
    fn a_stroke_is_a_stroked_path() {
        let page = page_to_emf(&pdf("2 w 0 0 0 RG 10 10 m 150 150 l S", ""), 0).expect("draws");
        assert!(record_types(&page.bytes).contains(&EMR_STROKEPATH));
    }

    #[test]
    fn a_clip_saves_and_restores_the_device_context() {
        let page = page_to_emf(&pdf("q 0 0 100 100 re W n 0 0 1 rg 0 0 200 200 re f Q", ""), 0).expect("draws");
        let types = record_types(&page.bytes);
        assert!(types.contains(&EMR_SELECTCLIPPATH));
        let saves = types.iter().filter(|&&t| t == EMR_SAVEDC).count();
        let restores = types.iter().filter(|&&t| t == EMR_RESTOREDC).count();
        assert_eq!((saves, restores), (1, 1), "one save for the clip, one restore");
    }

    #[test]
    fn an_image_is_stretched_as_a_bitmap() {
        // A 2 x 2 RGB image, uncompressed, placed 100 points square.
        let resources = "/XObject << /Im1 5 0 R >>";
        let content = "q 100 0 0 100 50 50 cm /Im1 Do Q";
        let image = "5 0 obj << /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 12 >> stream\n\u{0}\u{0}\u{0}\u{ff}\u{0}\u{0}\u{0}\u{ff}\u{0}\u{0}\u{0}\u{ff}\u{ff}\u{ff}\u{ff}\nendstream endobj";
        let mut bytes = pdf(content, resources);
        // Splice the image object in before the trailer.
        let at = bytes.windows(8).position(|w| w == b"trailer ").expect("trailer");
        bytes.splice(at..at, image.bytes());
        let page = page_to_emf(&bytes, 0).expect("draws");
        assert!(record_types(&page.bytes).contains(&EMR_STRETCHDIBITS));
    }

    #[test]
    fn a_translucent_fill_is_refused_so_the_caller_can_fall_back() {
        let resources = "/ExtGState << /G1 << /ca 0.5 >> >>";
        let content = "/G1 gs 0 0 1 rg 10 10 50 50 re f";
        assert_eq!(page_to_emf(&pdf(content, resources), 0), Err(EmfError::Unsupported("a translucent colour")));
    }

    #[test]
    fn a_shading_fill_is_refused() {
        let resources = "/Shading << /Sh1 << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 200 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [0 0 0] /C1 [1 1 1] /N 1 >> >> >>";
        let content = "q 0 0 200 200 re W n /Sh1 sh Q";
        assert!(matches!(page_to_emf(&pdf(content, resources), 0), Err(EmfError::Unsupported(_))));
    }

    #[test]
    fn the_page_count_is_read() {
        assert_eq!(page_count(&pdf("0 0 0 rg", "")), Ok(1));
    }
}
