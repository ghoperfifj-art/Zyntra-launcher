//! Builds the app icon from assets/logo.webp: embeds a multi-size .ico into the .exe
//! and writes the "Z" mark as raw RGBA for the window icon and sidebar logo.

use image::{imageops, imageops::FilterType, Rgba, RgbaImage};
use std::path::{Path, PathBuf};

const MARK_SIZE: u32 = 128;

/// The "Z" with its swoosh on a transparent square (alpha keyed from brightness), centred.
fn glyph(logo: &RgbaImage) -> RgbaImage {
    let (w, h) = logo.dimensions();
    let s = |v: f32, total: u32| (v * total as f32 / 1254.0) as u32;
    // Bounding box of the bright "Z" + swoosh, inside the frame and above the wordmark.
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    // Brightness-weighted centre: the visual middle of the letter, which the thin swoosh
    // would otherwise pull to one side.
    let (mut cx, mut cy, mut weight) = (0.0f64, 0.0f64, 0.0f64);
    for y in s(170.0, h)..s(830.0, h) {
        for x in s(130.0, w)..s(1125.0, w) {
            let [r, g, b, _] = logo.get_pixel(x, y).0;
            let sum = r as u32 + g as u32 + b as u32;
            if sum > 330 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
                let wgt = (sum - 330) as f64;
                cx += x as f64 * wgt;
                cy += y as f64 * wgt;
                weight += wgt;
            }
        }
    }
    let (cx, cy) = ((cx / weight) as i64, (cy / weight) as i64);
    // Square around the visual centre, big enough for the whole mark plus a margin.
    let half = [cx - x0 as i64, x1 as i64 - cx, cy - y0 as i64, y1 as i64 - cy].into_iter().max().unwrap();
    let side = (half as f32 * 2.0 * 1.16) as u32;
    // Take the artwork as-is (glow, swoosh, pixel squares) with a margin around the Z, staying
    // inside the frame and above the wordmark, and feather its edges to transparent so it
    // melts into the icon background with no visible rectangle.
    let margin = ((x1 - x0) as f32 * 0.09) as u32;
    let (fx0, fx1) = (x0.saturating_sub(margin).max(s(112.0, w)), (x1 + margin).min(s(1142.0, w)));
    let (fy0, fy1) = (y0.saturating_sub(margin).max(s(150.0, h)), (y1 + margin).min(s(855.0, h)));
    let mut crop = imageops::crop_imm(logo, fx0, fy0, fx1 - fx0 + 1, fy1 - fy0 + 1).to_image();
    let (cw, ch) = (crop.width() as f32, crop.height() as f32);
    let feather = margin as f32 * 1.3;
    for (x, y, px) in crop.enumerate_pixels_mut() {
        let edge = (x as f32).min(cw - 1.0 - x as f32).min(y as f32).min(ch - 1.0 - y as f32);
        let t = (edge / feather).clamp(0.0, 1.0);
        px.0[3] = (t * t * (3.0 - 2.0 * t) * 255.0) as u8;
    }
    let mut square = RgbaImage::new(side, side);
    let ox = side as i64 / 2 - (cx - fx0 as i64);
    let oy = side as i64 / 2 - (cy - fy0 as i64);
    imageops::overlay(&mut square, &crop, ox, oy);
    square
}

/// App icon: the glyph on the logo's dark background, as a rounded square.
fn mark(logo: &RgbaImage, glyph: &RgbaImage) -> RgbaImage {
    let (w, h) = logo.dimensions();
    let bg = *logo.get_pixel(w * 40 / 1254, h * 40 / 1254);
    let mut square = RgbaImage::from_pixel(glyph.width(), glyph.height(), bg);
    imageops::overlay(&mut square, glyph, 0, 0);
    rounded(&square, 0.22)
}

/// Rounded-corner alpha mask with a 1px soft edge.
fn rounded(img: &RgbaImage, radius: f32) -> RgbaImage {
    let side = img.width() as f32;
    let r = side * radius;
    let mut out = img.clone();
    for (x, y, px) in out.enumerate_pixels_mut() {
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let qx = (fx - side / 2.0).abs() - (side / 2.0 - r);
        let qy = (fy - side / 2.0).abs() - (side / 2.0 - r);
        let d = qx.max(0.0).hypot(qy.max(0.0)) - r;
        let a = (0.5 - d * side / 256.0).clamp(0.0, 1.0);
        px.0[3] = (px.0[3] as f32 * a) as u8;
    }
    out
}

/// Writes a .ico with 32-bit BMP images.
fn write_ico(path: &Path, images: &[RgbaImage]) {
    let blobs: Vec<Vec<u8>> = images
        .iter()
        .map(|img| {
            let size = img.width();
            let mask_row = size.div_ceil(32) * 4;
            let mut bmp = Vec::new();
            for v in [40u32, size, size * 2] {
                bmp.extend_from_slice(&v.to_le_bytes());
            }
            bmp.extend_from_slice(&1u16.to_le_bytes());
            bmp.extend_from_slice(&32u16.to_le_bytes());
            bmp.extend_from_slice(&0u32.to_le_bytes());
            bmp.extend_from_slice(&(size * size * 4 + mask_row * size).to_le_bytes());
            bmp.extend_from_slice(&[0u8; 16]);
            for y in (0..size).rev() {
                for x in 0..size {
                    let Rgba([r, g, b, a]) = *img.get_pixel(x, y);
                    bmp.extend_from_slice(&[b, g, r, a]);
                }
            }
            bmp.extend(std::iter::repeat_n(0u8, (mask_row * size) as usize));
            bmp
        })
        .collect();

    let mut ico = vec![0, 0, 1, 0];
    ico.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (img, blob) in images.iter().zip(&blobs) {
        let dim = if img.width() >= 256 { 0 } else { img.width() as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(blob.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += blob.len() as u32;
    }
    for blob in &blobs {
        ico.extend_from_slice(blob);
    }
    std::fs::write(path, ico).expect("write icon");
}

fn main() {
    println!("cargo:rerun-if-changed=assets/logo.webp");
    println!("cargo:rerun-if-changed=assets/version_block.png");
    println!("cargo:rerun-if-changed=build.rs");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let logo = image::open("assets/logo.webp").expect("assets/logo.webp").to_rgba8();
    let logo = imageops::resize(&logo, 1024, 1024, FilterType::Lanczos3);
    let mark = mark(&logo, &glyph(&logo));

    // Every size is the "Z" mark alone (no wordmark), same as inside the launcher.
    let sized = |img: &RgbaImage, n: u32| imageops::resize(img, n, n, FilterType::Lanczos3);
    let images: Vec<RgbaImage> = [16, 24, 32, 48, 64, 128, 256].into_iter().map(|n| sized(&mark, n)).collect();
    let ico = out.join("zyntra.ico");
    write_ico(&ico, &images);
    std::fs::write(out.join("logo_mark.rgba"), sized(&mark, MARK_SIZE).into_raw()).unwrap();
    sized(&mark, 256).save(out.join("logo_mark_preview.png")).unwrap();
    // Source for the macOS .icns (macos/build-app.sh), kept in the repo as macos/icon.png.
    sized(&mark, 1024).save(out.join("logo_mark_1024.png")).unwrap();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon(ico.to_str().unwrap());
        res.set("ProductName", "Zyntra Launcher");
        res.set("FileDescription", "Zyntra Launcher — лаунчер Minecraft");
        res.compile().expect("embed icon");
    }
    // The add-ons bundled with the launcher (mods-src/), each packed as a zip.
    pack_dir("mods-src/zyntra_crystals", &["assets", "data"], &["horizon-mod.toml"], &out.join("zyntra_crystals.zip"));
    pack_dir("mods-src/zyntra_xray", &["assets"], &["pack.mcmeta"], &out.join("zyntra_xray.zip"));
}

/// Zips the given folders and top-level files of `root` (paths inside the zip relative to it).
fn pack_dir(root: &str, dirs: &[&str], top_files: &[&str], target: &Path) {
    use std::io::Write;
    println!("cargo:rerun-if-changed={root}");
    let root = Path::new(root);
    let mut files = Vec::new();
    let mut stack: Vec<PathBuf> = dirs.iter().map(|d| root.join(d)).collect();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())).flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.extend(top_files.iter().map(|f| root.join(f)));
    files.sort();
    let mut zip = zip::ZipWriter::new(std::fs::File::create(target).expect("bundled zip"));
    let options = zip::write::SimpleFileOptions::default();
    for path in files {
        let name = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        zip.start_file(name, options).unwrap();
        zip.write_all(&std::fs::read(&path).unwrap()).unwrap();
    }
    zip.finish().unwrap();
}
