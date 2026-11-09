//! Printing on Windows through GDI: each sheet is an enhanced metafile (see `pdfcraft-render`'s
//! `emf` module), played onto the printer's device context, one page at a time. The driver gets
//! drawing commands, so text and lines stay vectors.
//!
//! This is the only crate in the workspace that uses `unsafe`: every call is a Win32 function with
//! its own contract, wrapped here so the rest of the code sees a safe API. Everything else stays
//! under the workspace's `unsafe_code = "forbid"`.

#![cfg(windows)]

use windows::Win32::Foundation::{GetLastError, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateDCW, DEVMODE_COLLATE, DEVMODE_COLOR, DEVMODE_DUPLEX, DEVMODE_FIELD_FLAGS, DEVMODEW, DM_COLLATE, DM_COLOR, DM_COPIES, DM_DUPLEX,
    DM_IN_BUFFER, DM_OUT_BUFFER, DM_PAPERSIZE, DMCOLLATE_FALSE, DMCOLLATE_TRUE, DMCOLOR_COLOR, DMCOLOR_MONOCHROME, DMDUP_HORIZONTAL,
    DMDUP_SIMPLEX, DMDUP_VERTICAL, DeleteDC, DeleteEnhMetaFile, GetDeviceCaps, HDC, HORZRES, HORZSIZE, PHYSICALOFFSETX, PHYSICALOFFSETY,
    PlayEnhMetaFile, SetEnhMetaFileBits, VERTRES, VERTSIZE,
};
use windows::Win32::Graphics::Printing::{ClosePrinter, DocumentPropertiesW, GetDefaultPrinterW, OpenPrinterW, PRINTER_HANDLE};
use windows::Win32::Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};
use windows::core::{PCWSTR, PWSTR};

/// How the job's sheets are printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Duplex {
    /// The printer's own setting: the job does not change it.
    #[default]
    Printer,
    /// One side per sheet.
    Off,
    /// Two sides, turned on the long edge.
    LongEdge,
    /// Two sides, turned on the short edge.
    ShortEdge,
}

/// The settings of one print job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// `None` is the default printer.
    pub printer: Option<String>,
    /// Copies of the whole job.
    pub copies: u32,
    /// Whether the copies come out one whole job at a time.
    pub collate: bool,
    /// Two-sided printing.
    pub duplex: Duplex,
    /// Print in greyscale.
    pub grayscale: bool,
    /// The name the spooler lists the job under.
    pub title: String,
}

/// One sheet: the bytes of an enhanced metafile, with its size in points.
#[derive(Debug, Clone, PartialEq)]
pub struct Sheet {
    /// The metafile.
    pub emf: Vec<u8>,
    /// The sheet's width in points.
    pub width_pt: f64,
    /// The sheet's height in points.
    pub height_pt: f64,
}

/// Why a job could not be printed; the message is shown to the user.
pub type Error = String;

/// The name of the default printer, if there is one.
pub fn default_printer() -> Option<String> {
    // SAFETY: the first call only asks for the length; the buffer is then sized to that length
    // and the second call writes into it.
    unsafe {
        let mut len = 0u32;
        // Fails with the needed length in `len` when no buffer is given.
        let _ = GetDefaultPrinterW(None, &mut len);
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        GetDefaultPrinterW(Some(PWSTR(buf.as_mut_ptr())), &mut len).ok().ok()?;
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let name = String::from_utf16_lossy(&buf[..end]);
        (!name.is_empty()).then_some(name)
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The driver's DEVMODE for a printer, changed for the job. The buffer is `u32`-aligned, as a
/// `DEVMODEW` needs.
struct DevMode {
    words: Vec<u32>,
}

impl DevMode {
    fn ptr(&mut self) -> *mut DEVMODEW {
        self.words.as_mut_ptr().cast()
    }
}

/// Sets `flag` in a DEVMODE's field mask.
fn mark(fields: &mut DEVMODE_FIELD_FLAGS, flag: DEVMODE_FIELD_FLAGS) {
    fields.0 |= flag.0;
}

/// The printer's DEVMODE, with the job's copies, sides, colour, collation and paper applied.
fn devmode(name: &[u16], options: &Options, paper: Option<i16>) -> Result<DevMode, Error> {
    // SAFETY: `name` is a NUL-terminated wide string that lives across every call. The DEVMODE
    // buffer is allocated with the size the driver reports and filled by the driver before any
    // field is read or written. The printer handle is closed on every path.
    unsafe {
        let mut printer = PRINTER_HANDLE::default();
        OpenPrinterW(PCWSTR(name.as_ptr()), &mut printer, None).map_err(|e| format!("the printer is not available: {e}"))?;
        let result = (|| -> Result<DevMode, Error> {
            let size = DocumentPropertiesW(None, printer, PCWSTR(name.as_ptr()), None, None, 0);
            if size <= 0 {
                return Err("the printer gave no settings".into());
            }
            let mut dm = DevMode { words: vec![0u32; (size as usize).div_ceil(4)] };
            if DocumentPropertiesW(None, printer, PCWSTR(name.as_ptr()), Some(dm.ptr()), None, DM_OUT_BUFFER.0) < 0 {
                return Err("the printer gave no settings".into());
            }
            let fields = &mut *dm.ptr();
            fields.Anonymous1.Anonymous1.dmCopies = options.copies.clamp(1, 999) as i16;
            mark(&mut fields.dmFields, DM_COPIES);
            let sides: Option<DEVMODE_DUPLEX> = match options.duplex {
                Duplex::Printer => None,
                Duplex::Off => Some(DMDUP_SIMPLEX),
                Duplex::LongEdge => Some(DMDUP_VERTICAL),
                Duplex::ShortEdge => Some(DMDUP_HORIZONTAL),
            };
            if let Some(sides) = sides {
                fields.dmDuplex = sides;
                mark(&mut fields.dmFields, DM_DUPLEX);
            }
            let colour: DEVMODE_COLOR = if options.grayscale { DMCOLOR_MONOCHROME } else { DMCOLOR_COLOR };
            fields.dmColor = colour;
            mark(&mut fields.dmFields, DM_COLOR);
            let collate: DEVMODE_COLLATE = if options.collate { DMCOLLATE_TRUE } else { DMCOLLATE_FALSE };
            fields.dmCollate = collate;
            mark(&mut fields.dmFields, DM_COLLATE);
            if let Some(size) = paper {
                fields.Anonymous1.Anonymous1.dmPaperSize = size;
                mark(&mut fields.dmFields, DM_PAPERSIZE);
            }
            let flags = DM_IN_BUFFER.0 | DM_OUT_BUFFER.0;
            if DocumentPropertiesW(None, printer, PCWSTR(name.as_ptr()), Some(dm.ptr()), Some(dm.ptr()), flags) < 0 {
                return Err("the printer refused the job settings".into());
            }
            Ok(dm)
        })();
        let _ = ClosePrinter(printer);
        result
    }
}

/// The DEVMODE paper for a sheet of this size, when the driver names it (A4, Letter, Legal). The
/// sizes are compared with 2 points of tolerance, either way round.
fn paper_for(width_pt: f64, height_pt: f64) -> Option<i16> {
    let (w, h) = (width_pt.min(height_pt), width_pt.max(height_pt));
    let near = |a: f64, b: f64| (a - b).abs() <= 2.0;
    if near(w, 595.28) && near(h, 841.89) {
        Some(9) // DMPAPER_A4
    } else if near(w, 612.0) && near(h, 792.0) {
        Some(1) // DMPAPER_LETTER
    } else if near(w, 612.0) && near(h, 1008.0) {
        Some(5) // DMPAPER_LEGAL
    } else {
        None
    }
}

/// The sheet's size in device pixels of the printer, from its resolution and physical size.
fn device_size(dc: HDC, width_pt: f64, height_pt: f64) -> (i32, i32) {
    // SAFETY: `dc` is a valid printer device context for the whole call.
    unsafe {
        let px_x = f64::from(GetDeviceCaps(Some(dc), HORZRES)) / f64::from(GetDeviceCaps(Some(dc), HORZSIZE).max(1));
        let px_y = f64::from(GetDeviceCaps(Some(dc), VERTRES)) / f64::from(GetDeviceCaps(Some(dc), VERTSIZE).max(1));
        let mm_per_pt = 25.4 / 72.0;
        ((width_pt * mm_per_pt * px_x).round() as i32, (height_pt * mm_per_pt * px_y).round() as i32)
    }
}

/// Prints `sheets` as one job. `progress(sent, total)` is called after each sheet has been
/// played. Returns the spooler's note on success.
pub fn print(options: &Options, sheets: &[Sheet], progress: &mut dyn FnMut(usize, usize)) -> Result<String, Error> {
    let Some(first) = sheets.first() else {
        return Err("the print-ready PDF has no pages".into());
    };
    let name = match &options.printer {
        Some(name) => name.clone(),
        None => default_printer().ok_or("there is no default printer; choose a printer")?,
    };
    let name_w = wide(&name);
    let paper = paper_for(first.width_pt, first.height_pt);
    let mut dm = devmode(&name_w, options, paper)?;
    let title = wide(&options.title);

    // SAFETY: the device context comes from the DEVMODE above and is deleted on every path. The
    // document is ended only after every page was played, and aborted on any error. Each
    // metafile is deleted once its page is done. `title` and `name_w` outlive every call.
    unsafe {
        let dc = CreateDCW(PCWSTR::null(), PCWSTR(name_w.as_ptr()), PCWSTR::null(), Some(dm.ptr() as *const DEVMODEW));
        if dc.is_invalid() {
            return Err(format!("the printer is not available: {name} (reopen Print to refresh the list)"));
        }
        let result = print_pages(dc, sheets, &title, progress);
        let _ = DeleteDC(dc);
        result.map(|()| format!("spooled {} sheet(s)", sheets.len()))
    }
}

/// The page loop on a device context that is ready to print.
///
/// # Safety
/// `dc` must be a valid printer device context, and `title` a NUL-terminated wide string.
unsafe fn print_pages(dc: HDC, sheets: &[Sheet], title: &[u16], progress: &mut dyn FnMut(usize, usize)) -> Result<(), Error> {
    let info = DOCINFOW {
        cbSize: std::mem::size_of::<DOCINFOW>() as i32,
        lpszDocName: PCWSTR(title.as_ptr()),
        ..Default::default()
    };
    // SAFETY: `info` lives across the call and `dc` is valid by this function's contract.
    if unsafe { StartDocW(dc, &info) } <= 0 {
        return Err(last_error("the job could not be started"));
    }
    let total = sheets.len();
    for (index, sheet) in sheets.iter().enumerate() {
        // SAFETY: the bytes are a complete metafile for the duration of the call.
        let emf = unsafe { SetEnhMetaFileBits(&sheet.emf) };
        if emf.is_invalid() {
            // SAFETY: the document was started above and no page is open.
            unsafe { abort(dc) };
            return Err(last_error("a page could not be read"));
        }
        // SAFETY: `dc` is in a document; the page is ended below on every path.
        if unsafe { StartPage(dc) } <= 0 {
            // SAFETY: `emf` is a handle this function owns, and no page is open.
            unsafe {
                let _ = DeleteEnhMetaFile(Some(emf));
                abort(dc);
            }
            return Err(last_error("a page could not be started"));
        }
        // The sheet is placed at the printable area's corner, at its true size.
        // SAFETY: `dc` is valid.
        let (ox, oy) = unsafe { (GetDeviceCaps(Some(dc), PHYSICALOFFSETX), GetDeviceCaps(Some(dc), PHYSICALOFFSETY)) };
        let (w, h) = device_size(dc, sheet.width_pt, sheet.height_pt);
        let rect = RECT { left: ox, top: oy, right: ox + w, bottom: oy + h };
        // SAFETY: `emf` is a metafile handle and `rect` lives across the call.
        let played = unsafe { PlayEnhMetaFile(dc, emf, &rect) };
        let page_error = if played.as_bool() { None } else { Some(last_error("a page could not be drawn")) };
        // SAFETY: the page was started above, so it is ended here; the metafile is not used again.
        unsafe {
            let _ = EndPage(dc);
            let _ = DeleteEnhMetaFile(Some(emf));
        }
        if let Some(e) = page_error {
            // SAFETY: the document is still started; no page is open.
            unsafe { abort(dc) };
            return Err(e);
        }
        progress(index + 1, total);
    }
    // SAFETY: the document was started above and every page was ended.
    if unsafe { EndDoc(dc) } <= 0 {
        return Err(last_error("the job could not be finished"));
    }
    Ok(())
}

/// Ends the document, discarding its pages.
///
/// # Safety
/// `dc` must be a valid device context in a started document with no page open.
unsafe fn abort(dc: HDC) {
    // SAFETY: the caller guarantees `dc` is in a started document.
    unsafe {
        let _ = AbortDoc(dc);
    }
}

fn last_error(context: &str) -> Error {
    // SAFETY: reads the calling thread's last error; no other state is touched.
    let code = unsafe { GetLastError() };
    format!("{context} (Windows error {})", code.0)
}
