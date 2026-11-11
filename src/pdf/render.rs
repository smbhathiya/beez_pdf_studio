//! Page rasterisation for previews, using `hayro` (pure Rust, no DLLs).
//!
//! Each open document gets one worker thread that owns the parsed PDF. Requests are served
//! newest-first, so the pages the user is looking at right now are rendered before pages
//! that scrolled out of view a moment ago.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};

/// A request: 1-based page number and the longest side of the bitmap in pixels.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Request {
    pub page: u32,
    pub size: u32,
}

/// RGBA (premultiplied; pages are opaque so it equals straight alpha) bitmap of a page.
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct Rendered {
    pub path: PathBuf,
    pub request: Request,
    /// `None` when the page could not be rendered (damaged or encrypted file…).
    pub bitmap: Option<Bitmap>,
}

/// Start a worker for one file. Send requests through the returned sender; results arrive
/// on `results`, followed by a call to `wake` so the UI can repaint.
pub fn spawn_worker(
    path: PathBuf,
    results: Sender<Rendered>,
    wake: impl Fn() + Send + 'static,
) -> Sender<Request> {
    let (tx, rx) = mpsc::channel::<Request>();
    std::thread::Builder::new()
        .name(format!("render {}", path.display()))
        .spawn(move || worker(&path, rx, results, wake))
        .expect("spawning a thread");
    tx
}

fn worker(path: &Path, rx: Receiver<Request>, results: Sender<Rendered>, wake: impl Fn()) {
    let pdf = std::fs::read(path)
        .ok()
        .and_then(|bytes| Pdf::new(bytes).ok());
    let settings = InterpreterSettings::default();
    let render_settings = RenderSettings::default();

    let mut queue: Vec<Request> = Vec::new();
    let mut done: HashSet<Request> = HashSet::new();
    loop {
        // Block for work when idle, then grab everything else that is waiting.
        if queue.is_empty() {
            match rx.recv() {
                Ok(r) => queue.push(r),
                Err(_) => return, // the UI dropped this document
            }
        }
        loop {
            match rx.try_recv() {
                Ok(r) => queue.push(r),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        // Newest request first.
        let Some(req) = queue.pop() else { continue };
        if !done.insert(req) {
            continue;
        }

        let bitmap = pdf.as_ref().and_then(|pdf| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let pages = pdf.pages();
                let page = pages.get(req.page.checked_sub(1)? as usize)?;
                let (w, h) = page.render_dimensions();
                if w <= 0.0 || h <= 0.0 {
                    return None;
                }
                let scale = req.size as f32 / w.max(h);
                let cache = RenderCache::new();
                let pixmap = render(
                    page,
                    &cache,
                    &settings,
                    &render_settings,
                    &PixmapSettings {
                        x_scale: scale,
                        y_scale: scale,
                        bg_color: WHITE,
                    },
                );
                Some(Bitmap {
                    width: pixmap.width() as u32,
                    height: pixmap.height() as u32,
                    rgba: pixmap.data_as_u8_slice().to_vec(),
                })
            }))
            .ok()
            .flatten()
        });

        if results
            .send(Rendered {
                path: path.to_path_buf(),
                request: req,
                bitmap,
            })
            .is_err()
        {
            return;
        }
        wake();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_page() {
        let dir = std::env::temp_dir().join("utilbee_render_test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.pdf");
        crate::pdf::save(&mut crate::pdf::tests::sample(2), &file).unwrap();

        let (tx, rx) = mpsc::channel();
        let req = spawn_worker(file.clone(), tx, || {});
        req.send(Request { page: 2, size: 800 }).unwrap();
        req.send(Request { page: 9, size: 800 }).unwrap(); // does not exist
        let mut got = vec![rx.recv().unwrap(), rx.recv().unwrap()];
        got.sort_by_key(|r| r.request.page);

        let bmp = got[0].bitmap.as_ref().expect("page 2 renders");
        assert!(
            bmp.height.abs_diff(800) <= 1,
            "longest side matches the request"
        );
        assert_eq!(bmp.rgba.len(), (bmp.width * bmp.height * 4) as usize);
        // The page has black text on white: some pixels must be dark.
        assert!(bmp.rgba.chunks(4).any(|p| p[0] < 100), "text was drawn");
        assert!(got[1].bitmap.is_none(), "missing page reports failure");
    }
}
