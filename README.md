# UtilBee PDF Studio

A fast desktop app for working with PDFs, written in Rust. Everything runs on your own computer and no files are uploaded.

## Tools

| Group | Tool | What it does |
|---|---|---|
| Organize | **Page Organizer** | Load several PDFs as page tiles, then select, rotate, move, duplicate, delete or reverse pages and save |
| | **Merge PDFs** | Join files in any order, optionally taking only some pages of each (`1-3, 7, 9-end`) |
| | **Split / Extract** | Split into one file per page, every N pages or per range, or extract pages into one PDF |
| Optimize & Convert | **Compress PDF** | Remove unused objects, use object streams and Flate, re-encode and downscale images |
| | **Images to PDF** | Convert JPG, PNG, WEBP, GIF and BMP to PDF. Page sizes: A3, A4, A5, Letter, Legal or same as the image. JPEGs keep full quality |
| | **Extract Text** | Save a PDF's text as `.txt` |
| Edit | **Watermark** | Text stamp with size, color, opacity, angle, position, and a choice of over or behind the content |
| | **Page Numbers** | Formats such as `Page {n} of {total}`, with start number, skipping cover pages, and 7 positions |
| | **Link Editor** | Add website links or go-to-page links, and list or remove existing links |
| Document | **Properties & Metadata** | View document info and edit title, author, subject, keywords and more |
| Security | **Protect** | AES-128 encryption with an open password, owner password and permissions |
| | **Unlock** | Remove the password from a PDF you have the password for |

You can drag files onto the window, or open PDFs straight into the organizer: `util_bee.exe a.pdf b.pdf`.

## Build & run

```sh
cargo run --release          # run the app
cargo test                   # engine tests
cargo build --release        # → target/release/util_bee.exe (single file, no runtime needed)
```

## Project layout

```
src/main.rs           window setup
src/app.rs            user interface (egui): sidebar, one panel per tool, background jobs
src/pdf/mod.rs        shared helpers: page boxes, rotation-aware coordinates, overlays, ranges
src/pdf/assemble.rs   page-assembly engine behind merge, split, extract and organize
src/pdf/compress.rs   structural + image compression
src/pdf/images.rs     images → PDF
src/pdf/stamp.rs      watermarks & page numbers
src/pdf/links.rs      link annotations
src/pdf/meta.rs       metadata, text extraction, encryption
```

## Known limitations

- Page tiles show page shapes and numbers, not rendered previews. Rendering previews would need a PDF rasterizer such as pdfium.
- Merge, split and organize keep page content, annotations and links. They drop document-level features: bookmarks, form field definitions and attachments.
- Watermarks and page numbers use the built-in Helvetica font, which only covers Latin-1 characters.
