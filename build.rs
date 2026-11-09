//! Embeds the browser app (`web/`) into the binary: every file gzipped
//! (level 9, mtime 0, so builds are reproducible) and listed in
//! `$OUT_DIR/web_assets.rs`, which `src/web/assets.rs` includes. Dev-only
//! directories (tests, e2e, dev, node_modules), dotfiles and npm files are
//! left out. Without `web/index.html` a stub page is embedded.
//!
//! Cargo reruns this script when an embedded file or a walked directory
//! changes: one `rerun-if-changed` line each, never one for `web/` itself
//! (cargo scans a directory recursively, and `web/` holds `node_modules`).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use flate2::{Compression, GzBuilder};
use sha2::{Digest, Sha256};

const SKIP_DIRS: &[&str] = &["tests", "e2e", "dev", "node_modules"];
const SKIP_FILES: &[&str] = &["package.json", "package-lock.json"];
const STUB_INDEX: &str = "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>claudio</title></head><body><p>claudio web: this build has no browser app \
(web/index.html was missing at build time).</p></body></html>\n";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // A local build's label (`make dev` sets it, e.g. `+dev.20f0b49`); releases leave it empty.
    println!("cargo:rerun-if-env-changed=CLAUDIO_BUILD_LABEL");
    let label: String = std::env::var("CLAUDIO_BUILD_LABEL")
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
        .take(64)
        .collect();
    println!("cargo:rustc-env=CLAUDIO_BUILD_LABEL={label}");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let gz_dir = out.join("web");
    let _ = fs::remove_dir_all(&gz_dir);
    fs::create_dir_all(&gz_dir).expect("create OUT_DIR/web");

    let web = Path::new("web");
    let mut files = Vec::new();
    if web.is_dir() {
        walk(web, web, &mut files);
    }
    files.sort();
    for (_, path) in &files {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let mut inputs: Vec<(String, Vec<u8>)> = files
        .into_iter()
        .map(|(rel, path)| {
            let bytes = fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            (rel, bytes)
        })
        .collect();
    if !inputs.iter().any(|(rel, _)| rel == "index.html") {
        // Watch the missing page, so the real app is embedded once it is
        // there. Cargo reruns a script whose watched path does not exist on
        // every build; only a tree without the app pays for that.
        println!("cargo:rerun-if-changed=web/index.html");
        inputs.push(("index.html".to_owned(), STUB_INDEX.as_bytes().to_vec()));
    }

    let mut table = String::from("pub(crate) static ASSETS: &[Asset] = &[\n");
    let mut importmap = None;
    for (n, (rel, bytes)) in inputs.iter().enumerate() {
        if rel == "index.html" {
            importmap = importmap_sha256(bytes);
        }
        let gz_path = gz_dir.join(format!("{n}.gz"));
        let mut gz = GzBuilder::new()
            .mtime(0)
            .write(Vec::new(), Compression::best());
        gz.write_all(bytes).expect("gzip");
        fs::write(&gz_path, gz.finish().expect("gzip")).expect("write gz");
        let digest = Sha256::digest(bytes);
        let etag: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
        table.push_str(&format!(
            "    Asset {{ path: {:?}, mime: {:?}, etag: \"\\\"{etag}\\\"\", gz: include_bytes!({:?}) }},\n",
            format!("/{rel}"),
            mime(rel),
            gz_path.display().to_string(),
        ));
    }
    table.push_str("];\n");
    table.push_str(&format!(
        "pub(crate) const IMPORTMAP_SHA256: Option<&str> = {:?};\n",
        importmap
    ));
    fs::write(out.join("web_assets.rs"), table).expect("write web_assets.rs");
}

/// Every file under `dir`, as (path relative to `root` with `/`, path).
/// Each walked directory but `root` gets a `rerun-if-changed` line here
/// (a file added to or removed from it); skipped ones never do.
fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    if dir != root {
        println!("cargo:rerun-if-changed={}", dir.display());
    }
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("UTF-8 file names under web/");
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            let top_level_dev = dir == root && SKIP_DIRS.contains(&name);
            if !top_level_dev && name != "node_modules" {
                walk(root, &path, out);
            }
        } else if !SKIP_FILES.contains(&name) {
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .expect("UTF-8 path");
            out.push((rel.replace('\\', "/"), path.clone()));
        }
    }
}

/// The CSP hash of `index.html`'s `<script type="importmap">` text.
/// Keep in step with `src/web/assets.rs`'s copy.
fn importmap_sha256(html: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(html).ok()?;
    let open = "<script type=\"importmap\">";
    let start = text.find(open)? + open.len();
    let end = start + text[start..].find("</script>")?;
    Some(
        base64::engine::general_purpose::STANDARD
            .encode(Sha256::digest(&text.as_bytes()[start..end])),
    )
}

/// Keep in step with `src/web/assets.rs`'s copy.
fn mime(rel: &str) -> &'static str {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let ext = match name.rsplit_once('.') {
        Some((_, ext)) => ext,
        None => "",
    };
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "txt" | "md" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
