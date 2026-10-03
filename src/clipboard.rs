//! The clipboard follows you: text, images and copied files move at the moment you switch.

use crate::proto::Clip;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

fn hash(c: &Clip) -> u64 {
    let mut h = DefaultHasher::new();
    match c {
        Clip::Text(t) => t.hash(&mut h),
        Clip::Image(b) => b.hash(&mut h),
        Clip::Files(f) => f.iter().for_each(|(n, b)| (n, b.len(), &b[..b.len().min(4096)]).hash(&mut h)),
    }
    h.finish()
}

pub struct Clipboard {
    last: Option<u64>,
}

/// Copied files are identified by path, size and modification time, so an unchanged
/// selection isn't read from disk again at every switch.
fn files_key(files: &[PathBuf]) -> u64 {
    let mut h = DefaultHasher::new();
    "files".hash(&mut h);
    for p in files {
        p.hash(&mut h);
        if let Ok(m) = p.metadata() {
            m.len().hash(&mut h);
            m.modified().ok().hash(&mut h);
        }
    }
    h.finish()
}

impl Clipboard {
    pub fn new() -> Self {
        Clipboard { last: None }
    }

    /// What's on the clipboard, if it changed since we last sent or received it.
    pub fn take_new(&mut self, max_files_bytes: u64) -> Option<Clip> {
        let mut cb = arboard::Clipboard::new().ok()?;
        if let Some(files) = cb.get().file_list().ok().filter(|f| !f.is_empty()) {
            let key = files_key(&files);
            if self.last == Some(key) {
                return None;
            }
            self.last = Some(key);
            return read_files(&files, max_files_bytes);
        }
        let clip = read_other(&mut cb)?;
        let h = hash(&clip);
        if self.last == Some(h) {
            return None;
        }
        self.last = Some(h);
        Some(clip)
    }

    pub fn apply(&mut self, clip: Clip) {
        // Mark it seen before writing, so it doesn't bounce straight back.
        self.last = Some(hash(&clip));
        match write(clip) {
            Ok(Some(paths)) => self.last = Some(files_key(&paths)),
            Ok(None) => {}
            Err(e) => log::warn!("could not set clipboard: {e}"),
        }
    }
}

fn read_files(files: &[PathBuf], max_files_bytes: u64) -> Option<Clip> {
    let total: u64 = files.iter().filter_map(|p| p.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum();
    if total > max_files_bytes {
        log::info!("copied files are {} MB; over the limit, not sending", total / 1_000_000);
        return None;
    }
    let out: Vec<(String, Vec<u8>)> = files
        .iter()
        .filter(|p| p.is_file())
        .filter_map(|p| Some((p.file_name()?.to_string_lossy().into_owned(), std::fs::read(p).ok()?)))
        .collect();
    (!out.is_empty()).then_some(Clip::Files(out))
}

fn read_other(cb: &mut arboard::Clipboard) -> Option<Clip> {
    if let Ok(img) = cb.get_image() {
        let mut png_bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png_bytes, img.width as u32, img.height as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_compression(png::Compression::Fast);
            let mut w = enc.write_header().ok()?;
            w.write_image_data(&img.bytes).ok()?;
        }
        return Some(Clip::Image(png_bytes));
    }
    cb.get_text().ok().filter(|t| !t.is_empty()).map(Clip::Text)
}

pub fn inbox() -> PathBuf {
    dirs::download_dir().unwrap_or_else(|| dirs::home_dir().unwrap()).join("Bridge")
}

/// Returns the local paths when files were placed on the clipboard.
fn write(clip: Clip) -> Result<Option<Vec<PathBuf>>, String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    match clip {
        Clip::Text(t) => cb.set_text(t).map(|_| None).map_err(|e| e.to_string()),
        Clip::Image(png_bytes) => {
            let dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
            let mut r = dec.read_info().map_err(|e| e.to_string())?;
            let mut buf = vec![0; r.output_buffer_size()];
            let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
            if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
                return Err("unexpected image format".into());
            }
            buf.truncate(info.buffer_size());
            cb.set_image(arboard::ImageData {
                width: info.width as usize,
                height: info.height as usize,
                bytes: buf.into(),
            })
            .map(|_| None)
            .map_err(|e| e.to_string())
        }
        Clip::Files(files) => {
            let dir = inbox();
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let mut paths = Vec::new();
            for (name, data) in files {
                let p = unique(&dir, &safe_name(&name));
                std::fs::write(&p, data).map_err(|e| e.to_string())?;
                paths.push(p);
            }
            cb.set().file_list(&paths).map_err(|e| e.to_string())?;
            Ok(Some(paths))
        }
    }
}

/// A file name from the other machine, made safe to create here on either OS:
/// no path separators or reserved characters, no reserved Windows device names, no trailing dots.
pub fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim_end_matches(['.', ' ']).to_string();
    let stem = cleaned.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    let cleaned: String = cleaned.chars().take(200).collect();
    if cleaned.is_empty() || cleaned.chars().all(|c| c == '.') {
        "file".into()
    } else if reserved {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}

fn unique(dir: &std::path::Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (2..).map(|i| dir.join(format!("{stem} {i}{ext}"))).find(|p| !p.exists()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::safe_name;

    #[test]
    fn names_cannot_escape_or_break() {
        assert_eq!(safe_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_name("..\\..\\Windows\\x.dll"), "_.._Windows_x.dll");
        assert_eq!(safe_name(".."), "file");
        assert_eq!(safe_name("CON.txt"), "_CON.txt");
        assert_eq!(safe_name("com1"), "_com1");
        assert_eq!(safe_name("a<b>:c?.txt. "), "a_b__c_.txt");
        assert_eq!(safe_name("report.pdf"), "report.pdf");
        assert_eq!(safe_name("photo\u{0}.jpg"), "photo_.jpg");
    }
}
