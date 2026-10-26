//! Export a PDF ▸ Excel (.xlsx). UniPDF addition: every detected table keeps its rows and
//! columns, paragraphs go in column A, and each page starts after a blank row. Numbers written
//! the Brazilian way ("1.234,56") or the English way become real numbers.

use rust_xlsxwriter::{Format, Workbook};

use crate::{Item, Page, items};

/// "79,2234" → 79.2234, "1.234,5" → 1234.5, "12.5" → 12.5; anything else stays text.
fn number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit() || matches!(c, ',' | '.' | '-' | '+')) {
        return None;
    }
    let normalized = if t.contains(',') { t.replace('.', "").replace(',', ".") } else { t.to_string() };
    normalized.parse().ok()
}

pub fn xlsx(pages: &[Page]) -> Result<Vec<u8>, String> {
    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    let bold = Format::new().set_bold();
    let mut row: u32 = 0;
    let mut widths: Vec<usize> = Vec::new();
    let mut put = |sheet: &mut rust_xlsxwriter::Worksheet, row: u32, col: u16, text: &str, is_bold: bool| -> Result<(), String> {
        let w = widths.len().max(col as usize + 1);
        widths.resize(w, 0);
        widths[col as usize] = widths[col as usize].max(text.chars().count().min(60));
        let r = match number(text) {
            Some(n) => sheet.write_number(row, col, n).map(|_| ()),
            None if is_bold => sheet.write_string_with_format(row, col, text, &bold).map(|_| ()),
            None => sheet.write_string(row, col, text).map(|_| ()),
        };
        r.map_err(|e| e.to_string())
    };
    for item in items(pages) {
        match item {
            Item::Para(b, _) => {
                put(sheet, row, 0, &b.text.replace('\n', " "), b.bold)?;
                row += 1;
            }
            Item::Table(t) => {
                for cells in &t.rows {
                    let mut col: u16 = 0;
                    for c in cells {
                        put(sheet, row, col, &c.text.replace('\n', " "), c.bold)?;
                        col += c.span.max(1) as u16;
                    }
                    row += 1;
                }
                row += 1;
            }
            Item::PageBreak => row += 1,
            Item::Img(_) => {}
        }
    }
    for (c, w) in widths.iter().enumerate() {
        sheet.set_column_width(c as u16, (*w as f64 + 2.0).max(8.0)).map_err(|e| e.to_string())?;
    }
    book.save_to_buffer().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::number;

    #[test]
    fn brazilian_and_english_numbers() {
        assert_eq!(number("79,2234"), Some(79.2234));
        assert_eq!(number("1.234,5"), Some(1234.5));
        assert_eq!(number("12.5"), Some(12.5));
        assert_eq!(number("mm"), None);
        assert_eq!(number(">500"), None);
    }
}
