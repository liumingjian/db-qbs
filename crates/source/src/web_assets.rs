use std::borrow::Cow;

use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist/"]
struct WebAssets;

pub struct EmbeddedWebAsset {
    pub body: Cow<'static, [u8]>,
    pub content_type: &'static str,
}

pub fn embedded_web_asset(request_path: &str) -> Option<EmbeddedWebAsset> {
    let path = request_path
        .split(['?', '#'])
        .next()
        .unwrap_or(request_path);
    let asset_path = match path {
        "/" | "/index.html" => "index.html",
        path if path.starts_with('/') && !path.contains("..") => &path[1..],
        _ => return None,
    };
    let file = WebAssets::get(asset_path)?;
    Some(EmbeddedWebAsset {
        body: file.data,
        content_type: content_type(asset_path),
    })
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}
