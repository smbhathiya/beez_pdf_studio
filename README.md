# 🐝 UtilBee PDF Studio

A fast desktop app for working with PDFs, written in Rust. Everything runs on your own computer and no files are uploaded.

---

## Features

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

Other conveniences:

- Drag files onto the window to add them to the current tool.
- Open PDFs straight into the organizer: `util_bee.exe a.pdf b.pdf`
- Work runs in the background, so the window never freezes. When a save finishes, the status bar offers **Open file** and **Open folder** buttons.
- Dark and light themes.

---

## 1. Requirements

| | Windows | macOS | Linux |
|---|---|---|---|
| Rust | [rustup](https://rustup.rs) (stable, 1.85 or newer for edition 2024) | same | same |
| C/C++ toolchain | Visual Studio Build Tools with the **"Desktop development with C++"** workload | Xcode Command Line Tools (`xcode-select --install`) | `build-essential` |
| Extra libraries | none | none | see below |

On Debian or Ubuntu, the window and file-dialog libraries are usually:

```sh
sudo apt-get install libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
                     libxkbcommon-dev libwayland-dev libssl-dev
```

Check your installation:

```sh
rustc --version
cargo --version
```

---

## 2. Run the app (development)

```sh
cd util_bee
cargo run                 # debug build: compiles faster, runs slower
cargo run --release       # optimized build: recommended for big PDFs
```

To pass files on startup with `cargo run`, put them after `--`:

```sh
cargo run --release -- "C:\path\a.pdf" "C:\path\b.pdf"
```

The first build downloads and compiles all dependencies, which takes a few minutes. Later builds are much faster.

---

## 3. Test & check

```sh
cargo test                # PDF engine tests (merge, split, compress, links, encryption…)
cargo clippy              # lints
cargo fmt                 # format the code
```

Create small demo PDFs in `target/samples/` for trying out the app:

```sh
cargo test write_samples -- --ignored
```

---

## 4. Build the app

```sh
cargo build --release
```

The result is a single executable with no runtime and no DLLs to ship:

| OS | Output |
|---|---|
| Windows | `target\release\util_bee.exe` |
| macOS / Linux | `target/release/util_bee` |

In release builds on Windows, the console window is hidden, so the app opens like a normal desktop program. Debug builds keep the console so you can see log output.

### Optional: smaller executable

Add this to `Cargo.toml` to shrink the binary, at the cost of longer build times:

```toml
[profile.release]
lto = true
codegen-units = 1
strip = true
opt-level = 3
```

---

## 5. Create a release

### Step 1: Bump the version

Edit `version` in `Cargo.toml` (e.g. `0.1.0` → `0.2.0`), then run:

```sh
cargo test
cargo build --release
```

### Step 2: Package the executable

**Windows (PowerShell):**

```powershell
$v = "0.2.0"
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\release\util_bee.exe "dist\UtilBee-PDF-Studio.exe"
Copy-Item README.md dist\
Compress-Archive -Path dist\* -DestinationPath "UtilBee-PDF-Studio-v$v-windows-x64.zip" -Force
```

**macOS / Linux:**

```sh
v=0.2.0
mkdir -p dist && cp target/release/util_bee dist/utilbee-pdf-studio && cp README.md dist/
tar -czf "UtilBee-PDF-Studio-v$v-$(uname -s | tr A-Z a-z)-$(uname -m).tar.gz" -C dist .
```

### Step 3: Tag the version in git

```sh
git add -A
git commit -m "release: v0.2.0"
git tag v0.2.0
git push origin main --tags
```

### Step 4: Publish on GitHub (optional)

This needs a GitHub remote (`git remote add origin <url>`) and the [GitHub CLI](https://cli.github.com) (`gh auth login`):

```sh
gh release create v0.2.0 UtilBee-PDF-Studio-v0.2.0-windows-x64.zip \
  --title "UtilBee PDF Studio v0.2.0" \
  --generate-notes
```

Users download the zip, extract it and double-click `UtilBee-PDF-Studio.exe`. No installation is needed.

> **Windows SmartScreen:** unsigned executables show an "unknown publisher" warning the first time they run. Users can click **More info → Run anyway**. Code-signing the `.exe` removes the warning.

### Release checklist

- [ ] Version bumped in `Cargo.toml`
- [ ] `cargo test` passes
- [ ] `cargo clippy` is clean
- [ ] Release build tested by hand (open, merge, compress, save)
- [ ] Archive created and named with the version
- [ ] Git tag pushed and GitHub release published

---

## Project structure

```
util_bee/
├── Cargo.toml            dependencies & version
├── README.md
└── src/
    ├── main.rs           window setup (title, size, drag & drop)
    ├── app.rs            user interface (egui): sidebar, one panel per tool, background jobs
    └── pdf/
        ├── mod.rs        shared helpers: page boxes, rotation-aware coordinates, overlays, ranges
        ├── assemble.rs   page-assembly engine behind merge, split, extract and organize
        ├── compress.rs   structural + image compression
        ├── images.rs     images → PDF
        ├── stamp.rs      watermarks & page numbers
        ├── links.rs      link annotations
        └── meta.rs       metadata, text extraction, encryption
```

### Main dependencies

| Crate | Used for |
|---|---|
| [`eframe` / `egui`](https://github.com/emilk/egui) | Native GUI |
| [`lopdf`](https://github.com/J-F-Liu/lopdf) | Reading and writing PDFs |
| [`image`](https://github.com/image-rs/image) | Decoding and encoding images |
| [`rfd`](https://github.com/PolyMeilex/rfd) | Native open, save and folder dialogs |
| [`rayon`](https://github.com/rayon-rs/rayon) | Parallel image compression |
| [`anyhow`](https://github.com/dtolnay/anyhow) | Error handling |

---

## Known limitations

- Page tiles show page shapes and numbers, not rendered previews. Rendering previews would need a PDF rasterizer such as pdfium.
- Merge, split and organize keep page content, annotations and links. They drop document-level features: bookmarks, form field definitions and attachments.
- Watermarks and page numbers use the built-in Helvetica font, which only covers Latin-1 characters.
