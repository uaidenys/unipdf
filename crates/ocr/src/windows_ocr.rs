//! UniPDF: recognition with Windows' built-in OCR (`Windows.Media.Ocr`). It reads the languages
//! installed in Windows — Portuguese with its accents included — and needs no model files.

use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::core::HSTRING;

use crate::{Line, OcrError, Word};

/// Portuguese (Brazil) when Windows has it, otherwise the user's own languages.
pub fn engine() -> Option<OcrEngine> {
    Language::CreateLanguage(&HSTRING::from("pt-BR"))
        .ok()
        .filter(|l| OcrEngine::IsLanguageSupported(l).unwrap_or(false))
        .and_then(|l| OcrEngine::TryCreateFromLanguage(&l).ok())
        .or_else(|| OcrEngine::TryCreateFromUserProfileLanguages().ok())
}

pub fn recognize(engine: &OcrEngine, pixels: &[u8], width: u32, height: u32) -> Result<Vec<Line>, OcrError> {
    let err = |e: windows::core::Error| OcrError::Recognize(e.message().to_string());
    let max = OcrEngine::MaxImageDimension().map_err(err)?;
    if width > max || height > max {
        return Err(OcrError::Recognize(format!("a imagem passa de {max} pixels; reduza a resolução")));
    }
    let n = (width * height) as usize;
    // BGRA, as SoftwareBitmap wants it.
    let bgra: Vec<u8> = if pixels.len() == n * 4 {
        pixels.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect()
    } else if pixels.len() == n * 3 {
        pixels.as_chunks::<3>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect()
    } else {
        pixels.iter().flat_map(|&g| [g, g, g, 255]).collect()
    };
    let writer = DataWriter::new().map_err(err)?;
    writer.WriteBytes(&bgra).map_err(err)?;
    let buffer = writer.DetachBuffer().map_err(err)?;
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(&buffer, BitmapPixelFormat::Bgra8, width as i32, height as i32, BitmapAlphaMode::Premultiplied).map_err(err)?;
    let result = engine.RecognizeAsync(&bitmap).map_err(err)?.join().map_err(err)?;
    let mut out = Vec::new();
    for line in result.Lines().map_err(err)? {
        let mut words = Vec::new();
        for w in line.Words().map_err(err)? {
            let r = w.BoundingRect().map_err(err)?;
            let text = w.Text().map_err(err)?.to_string_lossy();
            if !text.trim().is_empty() {
                words.push(Word { text, rect: [r.X, r.Y, r.X + r.Width, r.Y + r.Height] });
            }
        }
        if !words.is_empty() {
            out.push(Line { words });
        }
    }
    Ok(out)
}
