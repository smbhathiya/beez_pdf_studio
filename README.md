# 🐝 Beez PDF Studio

A fast desktop app for working with PDFs, written in Rust. Everything runs on your own computer and no files are uploaded.

---

## Features

The app opens on a **Home** screen with tool cards. Pick a tool and it opens full-window, with a back button (or press **Esc**) to return. The design follows Windows 11: Segoe UI fonts, Fluent icons, settings-style option cards, an InfoBar for results, and light or dark mode.

| Group | Tool | What it does |
|---|---|---|
| Organize pages | **Page Organizer** | Load several PDFs as page tiles, then select, rotate, move, duplicate, delete or reverse pages and save |
| | **Merge PDFs** | Join files in any order, optionally taking only some pages of each (`1-3, 7, 9-end`) |
| | **Split & Extract** | Split into one file per page, every N pages or per range, or extract pages into one PDF |
| Optimize & convert | **Compress PDF** | Light, Balanced or Strong presets: remove unused objects, use object streams, re-encode and downscale images |
| | **Images to PDF** | Convert JPG, PNG, WEBP, GIF and BMP to PDF with lossless image optimization (see below) |
| | **Extract Text** | Save a PDF's text as `.txt` |
| Edit document | **Link Editor** | Add website links or go-to-page links, and list or remove existing links |
| | **Properties** | View document info and edit title, author, subject, keywords and more |
| Security | **Protect PDF** | AES-128 encryption with an open password, owner password and permissions |
| | **Unlock PDF** | Remove the password from a PDF you have the password for |

### How Images to PDF keeps files small

PDF files cannot contain WebP data, so the app applies the same lossless techniques WebP and PNG use, in forms PDF supports:

- **Prediction filters:** row-by-row PNG filters (Sub, Up, Average, Paeth). Each image is compressed several ways and the smallest result is kept.
- **Maximum Deflate** compression.
- **Automatic palette:** images with 256 colors or fewer are stored as 1-, 2-, 4- or 8-bit indexed color.
- **Grayscale detection:** color images that are really gray are stored with one channel.
- **Transparency** is kept as a soft mask, and dropped when the image is fully opaque.
- **JPEG photos** are embedded byte-for-byte, with no quality loss and no size increase.
- **EXIF rotation** from phone photos is applied.

There are also two optional settings:
- **Smaller file (JPEG):** sets a JPEG quality. If lossless storage would be smaller for an image (common for screenshots), the app keeps that image lossless instead.
- **Limit resolution:** downscales very large images before converting.

Images are processed in parallel. Example: three screenshot PNGs totalling 246 KB were embedded in 211 KB, pixel for pixel identical.

### Page previews

Every PDF tool shows real thumbnails of all pages, rendered in the app with [hayro](https://github.com/LaurenzV/hayro) (pure Rust, so no extra DLLs are needed):

- **Page Organizer:** page tiles show the actual pages, including any rotation you applied. **Drag** pages (or a selection) to reorder them, drag them onto the **delete zone** that appears at the bottom to remove them (or press **Delete**), and double-click a page to open it full size.
- **Merge:** a strip of pages under each file. Pages outside the page selection are faded. Drag a file by its **☰ handle** to change the merge order, or onto the delete zone to remove it.
- **Images to PDF:** a thumbnail grid of the images in page order. Drag to reorder, drop on the delete zone (or click **×**) to remove, and click an image to view it large.
- **Split & Extract:** each page shows which output file it goes to (`File 1`, `File 2`…), and extracted pages are highlighted.
- **Link Editor:** a visual editor. Pick a page in the list on the left, **drag on the large page to draw a link area**, then type the web address (or choose a page to jump to) in the panel on the right. Existing links show in red (click one to remove it) and new links in blue (drag a new box to move it).
- **Compress, Extract Text, Properties, Protect:** a grid of every page.
- **Page viewer:** click any page to open it large. Use ← / → for previous and next, + / − to zoom, and Esc to close.

Thumbnails render on a background thread, visible pages first, so even long documents open instantly.

Other conveniences:

- Drag files onto the window. On Home, PDFs open in the Page Organizer and images open in Images to PDF.
- Open files from the command line: `util_bee.exe a.pdf b.pdf`
- Work runs in the background, so the window never freezes. When a task finishes, the InfoBar offers **Open file** and **Show in folder** buttons.

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

## 5. Installer & icon

- **Icon:** [`assets/icon.ico`](assets/icon.ico) is embedded into `util_bee.exe` by [`build.rs`](build.rs) (Explorer, taskbar, Start menu, Task Manager), together with the product name and publisher (**Beez**). To redesign it, edit [`examples/make_icon.rs`](examples/make_icon.rs) and run `cargo run --example make_icon`.
- **Installer:** [`installer/utilbee.iss`](installer/utilbee.iss) is an [Inno Setup](https://jrsoftware.org/isinfo.php) script. The installer:
  - installs for the current user into `%LOCALAPPDATA%\Programs\Beez PDF Studio`, so no administrator rights are needed and self-update keeps working,
  - adds a Start menu shortcut, and optionally a Desktop shortcut and an **Open with** entry for PDF files,
  - registers an uninstaller under *Settings → Apps*,
  - upgrades an existing installation in place.

Build the installer locally (requires Inno Setup 6):

```powershell
cargo build --release
& "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe" /DAppVersion=0.1.1 installer\utilbee.iss
# → dist\Beez-PDF-Studio-Setup-v0.1.1.exe
```

The release workflow builds and attaches it automatically.

---

## 6. Create a release

Releases are built and published automatically by GitHub Actions
([.github/workflows/release.yml](.github/workflows/release.yml)) when you push a version tag.

### Step 1: Bump the version

Edit `version` in `Cargo.toml` (e.g. `0.1.0` → `0.2.0`), then check that everything passes:

```sh
cargo test
cargo build --release
```

### Step 2: Commit, tag and push

```sh
git add -A
git commit -m "release: v0.2.0"
git tag v0.2.0
git push origin master --tags
```

The tag **must** be `v` + the version in `Cargo.toml`. The workflow stops if they don't match.

### Step 3: Wait for the workflow

On the repository's **Actions** tab, the *Release* workflow:

1. runs the tests,
2. builds `util_bee.exe` on Windows,
3. zips it as `Beez-PDF-Studio-v0.2.0-x86_64-pc-windows-msvc.zip`,
4. builds the installer `Beez-PDF-Studio-Setup-v0.2.0.exe` with Inno Setup,
5. creates the GitHub Release with download notes and both files attached.

When the workflow finishes, installed apps offer the update the next time they start (see [Auto-update](#7-auto-update)).

### Manual release (without GitHub Actions)

To publish by hand, the asset name and zip layout must match what the updater expects:

```powershell
$v = "0.2.0"
Compress-Archive -Path target\release\util_bee.exe -DestinationPath "Beez-PDF-Studio-v$v-x86_64-pc-windows-msvc.zip" -Force
gh release create "v$v" "Beez-PDF-Studio-v$v-x86_64-pc-windows-msvc.zip" --title "Beez PDF Studio v$v" --generate-notes
```

- The file name must contain `x86_64-pc-windows-msvc`.
- `util_bee.exe` must be at the root of the zip. Don't rename it inside the zip.

> **Windows SmartScreen:** unsigned executables show an "unknown publisher" warning the first time they run. Users can click **More info → Run anyway**. Code-signing the `.exe` removes the warning.

### Release checklist

- [ ] Version bumped in `Cargo.toml`
- [ ] `cargo test` passes and `cargo clippy` is clean
- [ ] Release build tested by hand (open, merge, compress, save)
- [ ] Tag `vX.Y.Z` pushed
- [ ] Release workflow succeeded and the zip is attached to the release

---

## 7. Auto-update

The app updates itself from GitHub Releases ([src/updater.rs](src/updater.rs)):

1. **On start-up** it checks the latest release in the background. If the user is offline or nothing is newer, nothing is shown.
2. If a newer version exists, a dialog pops up: **"Update available"**, with **Update now**, **What's new** and **Later**.
3. **Update now** downloads the release zip in the background, replaces the running `.exe` in place, and the dialog then offers **Restart now**.
4. Users can also click **Check for updates** at the bottom of the Home screen at any time.

Requirements:

- **The repository must be public.** GitHub's API returns *404 Not Found* for a private repository's releases unless a token is sent, and a token must never be shipped inside the app. To keep the source code private, publish releases to a separate public repository and change `REPO_NAME` in `src/updater.rs`.
- The repository is set in `src/updater.rs` (`REPO_OWNER` / `REPO_NAME`).
- Versions are compared as semantic versions (`0.10.0` is newer than `0.9.0`).
- GitHub allows 60 unauthenticated API requests per hour per IP address. That is plenty for one check per app start.
- Set the environment variable `UTILBEE_NO_UPDATE_CHECK=1` to turn off the start-up check (handy while developing).
- The app must be in a folder the user can write to (Desktop, Downloads, `%LOCALAPPDATA%`…). Under `C:\Program Files` the replacement fails without admin rights, and the app then offers the download page instead.

---

## Project structure

```
util_bee/
├── Cargo.toml            dependencies & version
├── README.md
├── build.rs              embeds the icon and version info into the .exe
├── assets/               icon.ico / icon.png
├── examples/make_icon.rs generates the icon
├── installer/utilbee.iss Inno Setup installer script
├── .github/workflows/
│   └── release.yml   builds & publishes a release when a vX.Y.Z tag is pushed
└── src/
    ├── main.rs           window setup (title, size, drag & drop)
    ├── updater.rs        self-update from GitHub Releases
    ├── ui/
    │   ├── mod.rs        app shell: Home launcher, tool pages, InfoBar, background tasks
    │   ├── theme.rs      Windows 11 look: fonts, Fluent icons, colors, cards, buttons
    │   ├── update.rs     update banner (check, update now, restart)
    │   ├── preview.rs    page thumbnails, page grid, full-size page viewer
    │   └── tools.rs      one panel per tool
    └── pdf/
        ├── mod.rs        shared helpers: page boxes, rotation-aware coordinates, ranges
        ├── assemble.rs   page-assembly engine behind merge, split, extract and organize
        ├── compress.rs   structural + image compression
        ├── images.rs     images → PDF with lossless optimization
        ├── links.rs      link annotations
        ├── render.rs     page rendering for previews (hayro, background threads)
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
| [`self_update`](https://github.com/jaemk/self_update) | Checking GitHub Releases and replacing the executable |
| [`hayro`](https://github.com/LaurenzV/hayro) | Rendering page previews (pure Rust) |
| [`winresource`](https://github.com/BenjaminRi/winresource) | Embedding the icon into the .exe |
| [`flate2`](https://github.com/rust-lang/flate2-rs) | Maximum-level Deflate for images |
| [`anyhow`](https://github.com/dtolnay/anyhow) | Error handling |

---

## Windows compatibility

Runs on **Windows 10 and Windows 11 (64-bit)**.

- **No runtime to install:** the Microsoft C runtime is linked into `util_bee.exe` ([`.cargo/config.toml`](.cargo/config.toml)), so it does not need `VCRUNTIME140.dll`.
- **Graphics fallback:** the window is drawn with DirectX 12 / Vulkan when available. On older graphics cards, virtual machines or Remote Desktop, the app automatically restarts itself using **OpenGL**. If neither works, it shows an error message asking the user to update the graphics driver.
- To force OpenGL (for troubleshooting), set the environment variable `UTILBEE_RENDERER=gl`.
- On Windows 10 the icons come from *Segoe MDL2 Assets* instead of *Segoe Fluent Icons*. They look nearly identical.

## Known limitations

- Password-protected PDFs show no preview until they are unlocked. Very unusual PDF features may render slightly differently from Adobe Reader, but the saved files are not affected, because previews are display-only.
- Merge, split and organize keep page content, annotations and links. They drop document-level features: bookmarks, form field definitions and attachments.
- The Windows 11 fonts and icons are loaded from `C:\Windows\Fonts`. On other systems the app falls back to egui's built-in fonts, and the icons will not display.
