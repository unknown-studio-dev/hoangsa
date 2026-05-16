use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::Response,
};
use rust_embed::RustEmbed;

/// Embeds the React SPA's production build. The folder is created on first
/// `make ui` run; until then the placeholder index below is served via the
/// `RustEmbed` fallback path so the dev loop works without Node installed.
#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../hoangsa-ui-web/dist/"]
struct Assets;

const PLACEHOLDER_HTML: &str = r#"<!doctype html>
<html lang="vi">
  <head>
    <meta charset="utf-8" />
    <title>hoangsa UI</title>
    <style>
      body { font-family: -apple-system, system-ui, sans-serif; padding: 2rem; max-width: 720px; margin: 0 auto; }
      code { background: #f3f4f6; padding: 0.1rem 0.4rem; border-radius: 4px; }
      .pill { display: inline-block; background: #fef3c7; color: #92400e; padding: 0.2rem 0.6rem; border-radius: 999px; font-size: 0.8rem; }
    </style>
  </head>
  <body>
    <h1>hoangsa Config UI</h1>
    <p class="pill">SPA chưa build</p>
    <p>Server đang chạy. Để build SPA chạy:</p>
    <pre><code>make ui</code></pre>
    <p>Hoặc gọi API trực tiếp với token đính kèm URL của trang này.</p>
  </body>
</html>
"#;

/// Serve any embedded SPA file. Falls through to `index.html` for SPA
/// client-side routes (any path that isn't an actual file). When the SPA
/// hasn't been built yet, returns a placeholder explaining how to build.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let lookup = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = Assets::get(lookup) {
        let mime = mime_guess::from_path(lookup).first_or_octet_stream();
        return Response::builder()
            .header(header::CONTENT_TYPE, mime.as_ref())
            .body(Body::from(file.data.into_owned()))
            .expect("body builds");
    }

    // SPA fallback: any non-file path → index.html so React Router can handle it.
    if let Some(index) = Assets::get("index.html") {
        return Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(index.data.into_owned()))
            .expect("body builds");
    }

    if path.is_empty() || lookup == "index.html" {
        return Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(PLACEHOLDER_HTML))
            .expect("body builds");
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("not found"))
        .expect("body builds")
}
