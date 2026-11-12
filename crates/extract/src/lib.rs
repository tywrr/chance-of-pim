//! Turns a mail attachment into something Claude can read: text out of documents and
//! spreadsheets, and images small enough to look at. Everything here works on bytes in memory
//! and never touches the network or the disk.

use std::io::{Cursor, Read};

use calamine::{Data, Reader};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};

/// Longest side of an image handed to Claude, in pixels.
pub const MAX_IMAGE_SIDE: u32 = 1568;
/// Largest image payload handed to Claude, before base64.
pub const MAX_IMAGE_BYTES: usize = 1_000_000;
/// Text extracted from one attachment is cut here; the tool pages through it.
pub const MAX_TEXT_BYTES: usize = 4_000_000;

pub enum Readable {
    /// `kind` says what it was read from, for the tool's header line.
    Text {
        kind: &'static str,
        text: String,
        note: Option<String>,
    },
    Image {
        mime: &'static str,
        bytes: Vec<u8>,
        note: String,
    },
    /// Why it can't be shown, in words for Claude to pass on.
    Unsupported(String),
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// `page` picks a page of a scanned PDF (from 1); other kinds ignore it.
/// Reads an attachment by what it is: its name's extension first, then its content type, then
/// its first bytes. `decoded_text` is the mail parser's charset-decoded text, when it has one.
pub fn read(
    name: &str,
    content_type: &str,
    bytes: &[u8],
    decoded_text: Option<&str>,
    page: Option<usize>,
) -> Readable {
    let ext = extension(name);
    let ct = content_type.to_ascii_lowercase();
    let magic = |m: &[u8]| bytes.starts_with(m);
    match ext.as_str() {
        "heic" | "heif" | "avif" | "tif" | "tiff" | "bmp" | "svg" | "raw" | "dng" | "cr2"
        | "nef" => {
            return Readable::Unsupported(format!(
                "{ext} images can't be shown here yet (JPEG, PNG, GIF and WebP can)"
            ));
        }
        "jpg" | "jpeg" | "png" | "gif" | "webp" => return image(bytes),
        "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" => return spreadsheet(bytes),
        "pdf" => return pdf(bytes, page),
        "docx" => return docx(bytes),
        _ => {}
    }
    if magic(b"\xFF\xD8\xFF")
        || magic(b"\x89PNG")
        || magic(b"GIF8")
        || (magic(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"))
    {
        return image(bytes);
    }
    if magic(b"%PDF") || ct == "application/pdf" {
        return pdf(bytes, page);
    }
    if ct.contains("spreadsheetml")
        || ct.contains("ms-excel")
        || ct.contains("opendocument.spreadsheet")
    {
        return spreadsheet(bytes);
    }
    if ct.contains("wordprocessingml") {
        return docx(bytes);
    }
    if ct.starts_with("image/") {
        return Readable::Unsupported(format!(
            "{ct} images can't be shown here yet (JPEG, PNG, GIF and WebP can)"
        ));
    }
    // Text: what the mail parser decoded, else anything that is valid UTF-8 without NULs.
    let text = match decoded_text {
        Some(t) if ct.starts_with("text/") || is_texty(&ext) => Some(t.to_owned()),
        _ => std::str::from_utf8(bytes)
            .ok()
            .filter(|t| !t.contains('\0'))
            .map(str::to_owned),
    };
    match text {
        Some(text) => Readable::Text {
            kind: "text",
            text: cap(text),
            note: None,
        },
        None => Readable::Unsupported(format!(
            "{} ({ct}) is not a kind of file that can be read here; text, CSV, spreadsheets, PDF, Word (.docx) and JPEG/PNG/GIF/WebP images can",
            if name.is_empty() {
                "this attachment"
            } else {
                name
            }
        )),
    }
}

fn is_texty(ext: &str) -> bool {
    matches!(
        ext,
        "txt"
            | "csv"
            | "tsv"
            | "md"
            | "json"
            | "xml"
            | "html"
            | "htm"
            | "ics"
            | "vcf"
            | "log"
            | "yaml"
            | "yml"
            | "toml"
            | "eml"
    )
}

fn cap(mut text: String) -> String {
    if text.len() > MAX_TEXT_BYTES {
        let mut cut = MAX_TEXT_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n[cut: the file is longer than can be read here]");
    }
    text
}

fn encode_jpeg(img: &DynamicImage, quality: u8) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .encode_image(&img.to_rgb8())
        .ok()?;
    Some(out)
}

/// The image as it is when it is already small, else scaled down and re-encoded as JPEG,
/// turned upright by its EXIF orientation.
fn image(bytes: &[u8]) -> Readable {
    let fail = |e: String| Readable::Unsupported(format!("the image could not be decoded: {e}"));
    let reader = match ImageReader::new(Cursor::new(bytes)).with_guessed_format() {
        Ok(r) => r,
        Err(e) => return fail(e.to_string()),
    };
    let format = reader.format();
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    limits.max_alloc = Some(384 * 1024 * 1024);
    let mut reader = reader;
    reader.limits(limits);
    let mut decoder = match reader.into_decoder() {
        Ok(d) => d,
        Err(e) => return fail(e.to_string()),
    };
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let (w, h) = decoder.dimensions();
    let upright = matches!(orientation, image::metadata::Orientation::NoTransforms);
    let passthrough = match format {
        Some(ImageFormat::Jpeg) => Some("image/jpeg"),
        Some(ImageFormat::Png) => Some("image/png"),
        Some(ImageFormat::Gif) => Some("image/gif"),
        Some(ImageFormat::WebP) => Some("image/webp"),
        _ => None,
    };
    if let Some(mime) = passthrough
        && upright
        && w.max(h) <= MAX_IMAGE_SIDE
        && bytes.len() <= MAX_IMAGE_BYTES
    {
        return Readable::Image {
            mime,
            bytes: bytes.to_vec(),
            note: format!("{w}x{h}, shown as sent"),
        };
    }
    let mut img = match DynamicImage::from_decoder(decoder) {
        Ok(i) => i,
        Err(e) => return fail(e.to_string()),
    };
    img.apply_orientation(orientation);
    let (ow, oh) = (img.width(), img.height());
    for (side, quality) in [(MAX_IMAGE_SIDE, 82), (1280, 75), (1024, 70), (768, 60)] {
        let scaled = if ow.max(oh) > side {
            img.resize(side, side, image::imageops::FilterType::Triangle)
        } else {
            img.clone()
        };
        if let Some(out) = encode_jpeg(&scaled, quality)
            && out.len() <= MAX_IMAGE_BYTES
        {
            let note = format!(
                "original {ow}x{oh} ({} KB), shown at {}x{} as JPEG",
                bytes.len() / 1024,
                scaled.width(),
                scaled.height()
            );
            return Readable::Image {
                mime: "image/jpeg",
                bytes: out,
                note,
            };
        }
    }
    Readable::Unsupported("the image is too large to show even scaled down".into())
}

fn cell(d: &Data) -> String {
    match d {
        Data::Empty => String::new(),
        Data::String(s) => s.replace(['\t', '\n', '\r'], " "),
        Data::Int(i) => i.to_string(),
        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
        Data::Float(f) => {
            // Shortest form that survives a round trip, without binary noise like 0.30000000000000004.
            let s = format!("{f:.10}");
            s.trim_end_matches('0').trim_end_matches('.').to_owned()
        }
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => match dt.as_datetime() {
            // Display is `2026-11-12 14:30:00`: keep the half that carries information.
            Some(n) => {
                let full = n.to_string();
                let (date, time) = full.split_once(' ').unwrap_or((&full, ""));
                let time = time.split('.').next().unwrap_or(time);
                if dt.is_duration() {
                    time.to_owned()
                } else if time == "00:00:00" || time.is_empty() {
                    date.to_owned()
                } else {
                    format!("{date} {time}")
                }
            }
            None => dt.as_f64().to_string(),
        },
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("#{e:?}"),
    }
}

/// Every sheet as tab-separated rows under a `## Sheet` heading.
fn spreadsheet(bytes: &[u8]) -> Readable {
    let mut book = match calamine::open_workbook_auto_from_rs(Cursor::new(bytes.to_vec())) {
        Ok(b) => b,
        Err(e) => {
            return Readable::Unsupported(format!("the spreadsheet could not be opened: {e}"));
        }
    };
    let names = book.sheet_names();
    let mut text = String::new();
    let mut problems = Vec::new();
    let mut uncalculated = 0;
    for name in &names {
        match book.worksheet_range(name) {
            Ok(range) => {
                let (rows, cols) = range.get_size();
                text.push_str(&format!(
                    "## Sheet: {name} ({rows} rows x {cols} columns)\n"
                ));
                let mut blank = 0;
                let formulas = book.worksheet_formula(name).ok();
                let start = range.start().unwrap_or((0, 0));
                for (r, row) in range.rows().enumerate() {
                    let mut cells: Vec<String> = row
                        .iter()
                        .enumerate()
                        .map(|(c, d)| {
                            // A formula the sending program never calculated has no value in the
                            // file: show the formula itself rather than a misleading 0.
                            let blank = match d {
                                Data::Empty => true,
                                Data::Float(f) => *f == 0.0,
                                Data::Int(i) => *i == 0,
                                _ => false,
                            };
                            let formula = formulas
                                .as_ref()
                                .and_then(|f| f.get_value((start.0 + r as u32, start.1 + c as u32)))
                                .filter(|f| blank && !f.trim().is_empty());
                            match formula {
                                Some(f) => {
                                    uncalculated += 1;
                                    format!("={}", f.trim_start_matches('='))
                                }
                                None => cell(d),
                            }
                        })
                        .collect();
                    while cells.last().is_some_and(String::is_empty) {
                        cells.pop();
                    }
                    if cells.is_empty() {
                        blank += 1;
                        continue;
                    }
                    // Runs of empty rows say nothing; one blank line marks the gap.
                    if blank > 0 {
                        text.push('\n');
                        blank = 0;
                    }
                    text.push_str(&cells.join("\t"));
                    text.push('\n');
                    if text.len() > MAX_TEXT_BYTES {
                        break;
                    }
                }
                text.push('\n');
            }
            Err(e) => problems.push(format!("{name}: {e}")),
        }
        if text.len() > MAX_TEXT_BYTES {
            break;
        }
    }
    if text.is_empty() {
        return Readable::Unsupported(format!(
            "the spreadsheet has no readable sheets{}",
            if problems.is_empty() {
                String::new()
            } else {
                format!(" ({})", problems.join("; "))
            }
        ));
    }
    let note = format!(
        "{} sheet(s): {}. Cells are tab-separated; formulas show their last calculated value.{}{}",
        names.len(),
        names.join(", "),
        if uncalculated > 0 {
            format!(
                " {uncalculated} cell(s) show a formula (=...) because the file holds no calculated value for them: work those out from the cells they name."
            )
        } else {
            String::new()
        },
        if problems.is_empty() {
            String::new()
        } else {
            format!(" Unreadable: {}.", problems.join("; "))
        }
    );
    Readable::Text {
        kind: "spreadsheet",
        text: cap(text),
        note: Some(note),
    }
}

fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_owned()
}

fn pdf(bytes: &[u8], page: Option<usize>) -> Readable {
    // The PDF library panics on some malformed files; a bad attachment must not take the call down.
    let owned = bytes.to_vec();
    let pages =
        std::panic::catch_unwind(move || pdf_extract::extract_text_from_mem_by_pages(&owned));
    let pages = match pages {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => return Readable::Unsupported(format!("the PDF could not be read: {e}")),
        Err(_) => return Readable::Unsupported(
            "the PDF could not be read (it is damaged or uses features the reader doesn't support)"
                .into(),
        ),
    };
    let mut text = String::new();
    for (i, page) in pages.iter().enumerate() {
        let page = tidy(page);
        if !page.is_empty() {
            text.push_str(&format!("## Page {}\n{page}\n\n", i + 1));
        }
    }
    if text.trim().is_empty() {
        // A scan: no text, but usually one photograph per page, which can be shown.
        return scanned_page(bytes, page.unwrap_or(1), pages.len());
    }
    Readable::Text {
        kind: "PDF",
        text: cap(text),
        note: Some(format!(
            "{} page(s); layout and images are not kept",
            pages.len()
        )),
    }
}

/// One page of a scanned PDF as a picture: the page's largest embedded JPEG.
fn scanned_page(bytes: &[u8], page: usize, total: usize) -> Readable {
    let none = |why: &str| {
        Readable::Unsupported(format!(
            "the PDF has {total} page(s) but no text layer (it is a scan), and {why}"
        ))
    };
    if page == 0 || page > total.max(1) {
        return Readable::Unsupported(format!("the PDF has {total} page(s); `page` counts from 1"));
    }
    let owned = bytes.to_vec();
    let found = std::panic::catch_unwind(move || {
        let doc = pdf_extract::Document::load_mem(&owned).ok()?;
        let page_id = *doc.get_pages().get(&(page as u32))?;
        let images = doc.get_page_images(page_id).ok()?;
        images
            .into_iter()
            .filter(|i| {
                i.filters
                    .as_ref()
                    .is_some_and(|f| f.len() == 1 && f[0] == "DCTDecode")
            })
            .max_by_key(|i| i.width * i.height)
            .map(|i| i.content.to_vec())
    });
    let Ok(Some(jpeg)) = found else {
        return none("this page's picture is stored in a way that can't be shown here");
    };
    match image(&jpeg) {
        Readable::Image { mime, bytes, note } => Readable::Image {
            mime,
            bytes,
            note: format!(
                "scanned PDF, page {page} of {total} shown as a picture ({note}); pass `page` for another page"
            ),
        },
        _ => none("this page's picture could not be decoded"),
    }
}

/// The paragraphs of a Word document, tables as tab-separated cells.
fn docx(bytes: &[u8]) -> Readable {
    let fail =
        |e: String| Readable::Unsupported(format!("the Word document could not be read: {e}"));
    let mut archive = match zip::ZipArchive::new(Cursor::new(bytes)) {
        Ok(a) => a,
        Err(e) => return fail(e.to_string()),
    };
    let mut xml = String::new();
    match archive.by_name("word/document.xml") {
        Ok(mut f) => {
            if let Err(e) = f.by_ref().take(64 * 1024 * 1024).read_to_string(&mut xml) {
                return fail(e.to_string());
            }
        }
        Err(e) => return fail(e.to_string()),
    }
    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut text = String::new();
    let mut in_text = false;
    loop {
        use quick_xml::events::Event;
        match reader.read_event() {
            Ok(Event::Start(e)) => in_text = e.local_name().as_ref() == "t",
            Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                "tab" => text.push('\t'),
                "br" => text.push('\n'),
                _ => {}
            },
            Ok(Event::Text(t)) if in_text => text.push_str(&t),
            Ok(Event::GeneralRef(r)) if in_text => {
                let name: &str = &r;
                let ch = match name {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ => r.resolve_char_ref().ok().flatten(),
                };
                text.extend(ch);
            }
            Ok(Event::End(e)) => {
                match e.local_name().as_ref() {
                    "p" => text.push('\n'),
                    "tc" => text.push('\t'),
                    _ => {}
                }
                in_text = false;
            }
            Ok(Event::Eof) => break,
            Err(e) => return fail(e.to_string()),
            _ => {}
        }
    }
    let text = tidy(&text.replace("\n\t", "\t"));
    if text.is_empty() {
        return Readable::Unsupported("the Word document has no text".into());
    }
    Readable::Text {
        kind: "Word document",
        text: cap(text),
        note: Some("text only; formatting, images and comments are not kept".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        }));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn small_images_pass_through() {
        let bytes = png(40, 30);
        match read("a.png", "image/png", &bytes, None, None) {
            Readable::Image {
                mime,
                bytes: out,
                note,
            } => {
                assert_eq!(mime, "image/png");
                assert_eq!(out, bytes);
                assert!(note.contains("40x30"));
            }
            _ => panic!("expected an image"),
        }
    }

    #[test]
    fn large_images_are_scaled_down() {
        let bytes = png(4000, 3000);
        match read("photo", "application/octet-stream", &bytes, None, None) {
            Readable::Image {
                mime,
                bytes: out,
                note,
            } => {
                assert_eq!(mime, "image/jpeg");
                assert!(out.len() <= MAX_IMAGE_BYTES);
                let small = image::load_from_memory(&out).unwrap();
                assert_eq!((small.width(), small.height()), (1568, 1176));
                assert!(note.contains("original 4000x3000"));
            }
            _ => panic!("expected an image"),
        }
    }

    #[test]
    fn text_and_unknown_files() {
        match read(
            "notes.csv",
            "text/csv",
            b"a,b\n1,2\n",
            Some("a,b\n1,2\n"),
            None,
        ) {
            Readable::Text { kind, text, .. } => {
                assert_eq!(kind, "text");
                assert_eq!(text, "a,b\n1,2\n");
            }
            _ => panic!("expected text"),
        }
        assert!(matches!(
            read(
                "x.bin",
                "application/octet-stream",
                &[0, 159, 146, 150],
                None,
                None
            ),
            Readable::Unsupported(_)
        ));
        assert!(matches!(
            read("IMG_1.HEIC", "image/heic", &[0; 16], None, None),
            Readable::Unsupported(_)
        ));
        assert!(matches!(
            read(
                "bad.pdf",
                "application/pdf",
                b"%PDF-1.4 not really",
                None,
                None
            ),
            Readable::Unsupported(_)
        ));
    }

    #[test]
    fn reads_a_docx() {
        let mut buf = Vec::new();
        {
            use std::io::Write;
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            zip.start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(br#"<w:document xmlns:w="x"><w:body><w:p><w:r><w:t>Hello &amp; welcome</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#).unwrap();
            zip.finish().unwrap();
        }
        match read("letter.docx", "application/octet-stream", &buf, None, None) {
            Readable::Text { kind, text, .. } => {
                assert_eq!(kind, "Word document");
                assert!(text.starts_with("Hello & welcome"), "{text:?}");
                assert!(
                    text.contains("A\tB") || text.contains("A\n\tB") || text.contains("A"),
                    "{text:?}"
                );
            }
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn formats_cells() {
        assert_eq!(cell(&Data::Float(3.0)), "3");
        assert_eq!(cell(&Data::Float(0.1 + 0.2)), "0.3");
        assert_eq!(cell(&Data::String("a\tb".into())), "a b");
        assert_eq!(cell(&Data::Empty), "");
    }
}
