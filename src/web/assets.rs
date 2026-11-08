//! The browser app's files. `build.rs` embeds `web/` gzipped; a client that
//! accepts gzip gets the stored bytes, any other one a decompressed copy.
//! In a debug build, `CLAUDIO_WEB_DIR=<dir>` serves files from disk instead
//! (edit and reload, no rebuild).

#[cfg(debug_assertions)]
use std::path::Component;
use std::path::{Path, PathBuf};

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use sha2::{Digest, Sha256};

use super::AppState;

/// One embedded file.
pub(crate) struct Asset {
    /// The URL path, `/` first.
    pub path: &'static str,
    pub mime: &'static str,
    /// Quoted, as sent in `ETag`.
    pub etag: &'static str,
    /// The gzip bytes.
    pub gz: &'static [u8],
}

// `ASSETS` and `IMPORTMAP_SHA256`, written by build.rs.
include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));

/// Where the files come from, and the import map's CSP hash.
pub(crate) struct Assets {
    /// The canonical `CLAUDIO_WEB_DIR` (debug builds only).
    #[cfg(debug_assertions)]
    disk: Option<PathBuf>,
    importmap: Option<String>,
}

impl Assets {
    /// Embedded assets; in a debug build with `CLAUDIO_WEB_DIR` set, files
    /// from that directory instead (the import-map hash is read from its
    /// index.html at start: restart after editing the map). Blocking I/O in
    /// disk mode: call before serving.
    pub(crate) fn load() -> Assets {
        #[cfg(debug_assertions)]
        if let Some(dir) = std::env::var_os("CLAUDIO_WEB_DIR").filter(|d| !d.is_empty()) {
            match std::fs::canonicalize(&dir) {
                Ok(root) => {
                    let importmap = std::fs::read(root.join("index.html"))
                        .ok()
                        .and_then(|html| importmap_sha256(&html));
                    tracing::info!(dir = %root.display(), "web: serving the app from disk");
                    return Assets {
                        disk: Some(root),
                        importmap,
                    };
                }
                Err(e) => {
                    tracing::warn!(error = %e, "web: CLAUDIO_WEB_DIR unusable, serving the embedded app")
                }
            }
        }
        Assets {
            #[cfg(debug_assertions)]
            disk: None,
            importmap: IMPORTMAP_SHA256.map(str::to_owned),
        }
    }

    pub(crate) fn importmap_sha256(&self) -> Option<&str> {
        self.importmap.as_deref()
    }
}

/// The CSP hash (base64 SHA-256) of `index.html`'s
/// `<script type="importmap">` text. The same function as build.rs's.
#[cfg_attr(not(debug_assertions), allow(dead_code))]
pub(crate) fn importmap_sha256(html: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(html).ok()?;
    let open = "<script type=\"importmap\">";
    let start = text.find(open)? + open.len();
    let end = start + text[start..].find("</script>")?;
    Some(
        base64::engine::general_purpose::STANDARD
            .encode(Sha256::digest(text[start..end].as_bytes())),
    )
}

/// The `Content-Type` for a path, by extension. The same table as build.rs's.
#[cfg_attr(not(debug_assertions), allow(dead_code))]
pub(crate) fn mime(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
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

/// The file under `root` a URL path names, if it stays under `root`: every
/// `..`, `.` or empty segment is refused, and so is a path that leaves the
/// canonical `root` once symlinks are resolved. `root` is canonical.
#[cfg_attr(not(debug_assertions), allow(dead_code))]
pub(crate) fn disk_path(root: &Path, url_path: &str) -> Option<PathBuf> {
    let rel = url_path.strip_prefix('/')?;
    let mut path = root.to_path_buf();
    for seg in rel.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." || seg.contains('\\') || seg.contains('\0') {
            return None;
        }
        path.push(seg);
    }
    // The pushed segments are plain names (no separators were left in
    // them); resolve symlinks only when the file exists.
    match std::fs::canonicalize(&path) {
        Ok(real) if real.starts_with(root) => Some(real),
        Ok(_) => None,
        Err(_) => Some(path),
    }
}

/// Whether an `Accept-Encoding` value accepts gzip (a `q` of 0 refuses it).
pub(crate) fn accepts_gzip(accept_encoding: &str) -> bool {
    accept_encoding.split(',').any(|item| {
        let mut parts = item.split(';');
        let coding = parts.next().unwrap_or("").trim();
        if !coding.eq_ignore_ascii_case("gzip") && coding != "*" {
            return false;
        }
        let q = parts
            .filter_map(|p| {
                p.trim()
                    .strip_prefix("q=")
                    .or_else(|| p.trim().strip_prefix("Q="))
            })
            .next()
            .and_then(|q| q.trim().parse::<f32>().ok())
            .unwrap_or(1.0);
        q > 0.0
    })
}

fn find(path: &str) -> Option<&'static Asset> {
    ASSETS.iter().find(|a| a.path == path)
}

/// The asset fallback route: `GET`/`HEAD` of an app file (`/` is
/// `/index.html`).
pub(crate) async fn serve(
    #[cfg_attr(not(debug_assertions), allow(unused_variables))] State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let path = match uri.path() {
        "/" => "/index.html",
        other => other,
    };
    #[cfg(debug_assertions)]
    if let Some(root) = &state.assets.disk {
        return serve_disk(root, path).await;
    }
    let Some(asset) = find(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut res = if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|tag| tag.trim() == asset.etag))
    {
        StatusCode::NOT_MODIFIED.into_response()
    } else if headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(accepts_gzip)
    {
        let mut res = asset.gz.into_response();
        res.headers_mut()
            .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        res
    } else {
        let gz = asset.gz;
        match tokio::task::spawn_blocking(move || gunzip(gz)).await {
            Ok(Ok(bytes)) => bytes.into_response(),
            _ => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    };
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(asset.mime));
    h.insert(header::ETAG, HeaderValue::from_static(asset.etag));
    h.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    if asset.path != "/index.html" {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }
    res
}

fn gunzip(gz: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(gz).read_to_end(&mut out)?;
    Ok(out)
}

/// Debug disk mode: the file, uncompressed, read off the executor.
#[cfg(debug_assertions)]
async fn serve_disk(root: &Path, path: &str) -> Response {
    let Some(file) = disk_path(root, path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // Only plain names reached the path (see disk_path).
    debug_assert!(file
        .components()
        .all(|c| !matches!(c, Component::ParentDir)));
    let read = tokio::task::spawn_blocking(move || {
        std::fs::metadata(&file)
            .ok()
            .filter(|m| m.is_file())
            .and_then(|_| std::fs::read(&file).ok())
    })
    .await;
    match read {
        Ok(Some(bytes)) => {
            let mut res = bytes.into_response();
            let h = res.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime(path)));
            if path != "/index.html" {
                h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            }
            res
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_html() -> Vec<u8> {
        gunzip(find("/index.html").expect("an index").gz).unwrap()
    }

    #[test]
    fn every_asset_has_a_mime_an_etag_and_index_exists() {
        assert!(find("/index.html").is_some());
        for a in ASSETS {
            assert!(a.path.starts_with('/'), "{}", a.path);
            assert!(!a.mime.is_empty(), "{}", a.path);
            assert_eq!(a.mime, mime(a.path), "{}", a.path);
            assert!(
                a.etag.len() == 18 && a.etag.starts_with('"') && a.etag.ends_with('"'),
                "{}",
                a.etag
            );
            assert!(gunzip(a.gz).is_ok(), "{}", a.path);
            for skipped in ["/tests/", "/e2e/", "/dev/", "node_modules", "/."] {
                assert!(!a.path.contains(skipped), "{}", a.path);
            }
        }
    }

    #[test]
    fn the_import_map_hash_matches_the_embedded_index() {
        if let Some(hash) = IMPORTMAP_SHA256 {
            assert_eq!(importmap_sha256(&index_html()).as_deref(), Some(hash));
        } else {
            assert_eq!(importmap_sha256(&index_html()), None);
        }
        let html = b"<head><script type=\"importmap\">{\"imports\":{}}</script></head>";
        let want =
            base64::engine::general_purpose::STANDARD.encode(Sha256::digest(b"{\"imports\":{}}"));
        assert_eq!(importmap_sha256(html), Some(want));
        assert_eq!(importmap_sha256(b"<script>x</script>"), None);
    }

    #[test]
    fn mime_by_extension() {
        assert_eq!(mime("/index.html"), "text/html; charset=utf-8");
        assert_eq!(mime("/app/boot.js"), "text/javascript; charset=utf-8");
        assert_eq!(
            mime("/vendor/preact/preact.mjs"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(mime("/styles/base.css"), "text/css; charset=utf-8");
        assert_eq!(mime("/fonts/Inter-Regular.woff2"), "font/woff2");
        assert_eq!(mime("/icon.svg"), "image/svg+xml");
        assert_eq!(mime("/x.json"), "application/json");
        assert_eq!(mime("/manifest.webmanifest"), "application/manifest+json");
        assert_eq!(mime("/x.unknown"), "application/octet-stream");
        assert_eq!(mime("/noext"), "application/octet-stream");
        assert_eq!(mime("/dir.js/noext"), "application/octet-stream");
    }

    #[test]
    fn disk_paths_cannot_escape() {
        let root = crate::paths::socket_test_dir("web-disk");
        std::fs::create_dir_all(root.join("app")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        for bad in [
            "/../x",
            "/a/../../x",
            "/./x",
            "//etc/passwd",
            "/app/",
            "x",
            "/app\\..\\x",
        ] {
            assert_eq!(disk_path(&root, bad), None, "{bad}");
        }
        assert_eq!(
            disk_path(&root, "/app/boot.js"),
            Some(root.join("app/boot.js"))
        );
        // A symlink out of the root is refused once resolved.
        std::os::unix::fs::symlink("/etc", root.join("out")).unwrap();
        assert_eq!(disk_path(&root, "/out/passwd"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gzip_is_accepted_only_when_asked() {
        assert!(accepts_gzip("gzip, deflate"));
        assert!(accepts_gzip("br;q=1, gzip;q=0.5"));
        assert!(accepts_gzip("GZIP"));
        assert!(!accepts_gzip(""));
        assert!(!accepts_gzip("identity"));
        assert!(!accepts_gzip("gzip;q=0"));
        assert!(!accepts_gzip("br, gzip;q=0.0"));
    }
}
