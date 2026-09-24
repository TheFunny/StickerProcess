//! `preview://` 自定义协议（Phase D 双轨制的"大文件轨"）。
//!
//! WebView2 把名为 `preview` 的自定义协议映射为 `http://preview./…`，
//! 本模块提供 URL 构造与请求 handler：按扩展名给 MIME、支持 HTTP Range
//! （视频拖动必需）、按需读盘（配合 OS 页缓存，无需自建缓存层）。
//!
//! 安全性：URL 携带 percent-encoded 的本机路径，但该协议仅注册在本进程的
//! WebView 内，外部无法访问。

use dioxus::desktop::{Config, wry};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use wry::http::header::{CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE};
use wry::http::{Request as HttpRequest, Response as HttpResponse, StatusCode};

/// Windows 下 wry 对自定义协议的 workaround 前缀（`{http}://{name}.`）。
pub const URL_PREFIX: &str = "http://preview.";

/// 为本地文件构造预览 URL。
pub fn media_url(path: &Path) -> String {
    format!(
        "{}media?p={}",
        URL_PREFIX,
        percent_encode(&path.to_string_lossy())
    )
}

/// 注册到 `Config`；读盘放在 tokio 阻塞线程，避免卡 UI。
pub fn register(config: Config) -> Config {
    config.with_asynchronous_custom_protocol("preview", |_id, request, responder| {
        tokio::spawn(async move {
            let response = tokio::task::spawn_blocking(move || handle(request))
                .await
                .unwrap_or_else(|_| not_found());
            responder.respond(response);
        });
    })
}

/// 协议入口：解析 query、构造响应。独立成同步函数便于测试。
fn handle(request: HttpRequest<Vec<u8>>) -> HttpResponse<Cow<'static, [u8]>> {
    let Some(path) = query_param(request.uri().query(), "p").map(PathBuf::from) else {
        return not_found();
    };
    let Some(mime) = mime_for(&path) else {
        return not_found();
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return not_found();
    };
    let total = meta.len();

    if let Some(value) = request.headers().get("range") {
        let range = value
            .to_str()
            .ok()
            .and_then(|header| parse_range(Some(header), total));
        return match range {
            Some((start, end)) => match read_slice(&path, start, end) {
                Some(data) => partial(mime, data, start, end, total),
                None => not_found(),
            },
            None => range_not_satisfiable(total),
        };
    }

    // wry 的自定义协议响应体必须完整交给 responder；无 Range 时也只接受
    // 一个有界窗口，避免伪装扩展名的大文件在打开预览时按全长分配。
    if total > MAX_WINDOW {
        return payload_too_large();
    }
    match std::fs::read(&path) {
        Ok(data) => full(mime, data),
        Err(_) => not_found(),
    }
}

type Body = Vec<u8>;

/// 单次响应窗口上限：无 Range 响应也受限，Range 命中则只返回该窗口。
/// Chromium/WebView2 对 `<video>` 常发 `bytes=0-`；图片没有 Range 保证，
/// 因此同样不能走无界全量分配。
const MAX_WINDOW: u64 = 8 * 1024 * 1024;

fn full(mime: &'static str, data: Body) -> HttpResponse<Cow<'static, [u8]>> {
    HttpResponse::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, mime)
        .header(CONTENT_LENGTH, data.len() as u64)
        .body(Cow::Owned(data))
        .unwrap()
}

fn partial(
    mime: &'static str,
    data: Body,
    start: u64,
    end: u64,
    total: u64,
) -> HttpResponse<Cow<'static, [u8]>> {
    HttpResponse::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(CONTENT_TYPE, mime)
        .header(CONTENT_LENGTH, end - start + 1)
        .header(CONTENT_RANGE, format!("bytes {start}-{end}/{total}"))
        .body(Cow::Owned(data))
        .unwrap()
}

fn error(status: StatusCode) -> HttpResponse<Cow<'static, [u8]>> {
    HttpResponse::builder()
        .status(status)
        .body(Cow::Borrowed(&b"request failed"[..]))
        .unwrap()
}

fn not_found() -> HttpResponse<Cow<'static, [u8]>> {
    error(StatusCode::NOT_FOUND)
}

fn payload_too_large() -> HttpResponse<Cow<'static, [u8]>> {
    error(StatusCode::PAYLOAD_TOO_LARGE)
}

fn range_not_satisfiable(total: u64) -> HttpResponse<Cow<'static, [u8]>> {
    HttpResponse::builder()
        .status(StatusCode::RANGE_NOT_SATISFIABLE)
        .header(CONTENT_RANGE, format!("bytes */{total}"))
        .body(Cow::Borrowed(&b"range not satisfiable"[..]))
        .unwrap()
}

/// 解析单段 `Range: bytes=start-end` / `bytes=start-`。
/// 无 Range 返回 None；非法、越界或多段 Range 也返回 None，由 handler 回答 416。
fn parse_range(header: Option<&str>, total: u64) -> Option<(u64, u64)> {
    let header = header?;
    let spec = header.strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start_s, end_s) = spec.split_once('-')?;
    let start: u64 = start_s.trim().parse().ok()?;
    if start >= total {
        return None;
    }
    let end = match end_s.trim().parse::<u64>() {
        Ok(end) => end.min(total - 1),
        Err(_) => total - 1, // open-ended "bytes=N-"
    };
    let end = end.min(start.saturating_add(MAX_WINDOW - 1));
    if end < start {
        return None;
    }
    Some((start, end))
}

fn read_slice(path: &Path, start: u64, end: u64) -> Option<Body> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(start)).ok()?;
    let len = usize::try_from(end - start + 1).ok()?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn mime_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    crate::media::mime_for_ext(&ext)
}

/// 极简 query 参数提取：`?key=value`（本协议足够）。
fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    for pair in query?.split('&') {
        if let Some((k, v)) = pair.split_once('=')
            && k == key
        {
            return Some(percent_decode(v));
        }
    }
    None
}

/// 编码保留集：RFC3986 unreserved（`-_.~` 已在 NON_ALPHANUMERIC 之外……不，
/// NON_ALPHANUMERIC 编码除字母数字外的一切，故逐个 remove 出不编码的字符）+
/// 路径现场需要的 `\` 与 `:`——URL 只在本进程 WebView 内往返，保持可读。
const PATH_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~')
    .remove(b'\\')
    .remove(b':');

pub fn percent_encode(raw: &str) -> String {
    utf8_percent_encode(raw, PATH_SET).to_string()
}

pub fn percent_decode(encoded: &str) -> String {
    percent_decode_str(encoded).decode_utf8_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_path_with_chinese_and_spaces() {
        let raw = r"D:\out put\目录\2026-08-25-214201.604.png";
        let encoded = percent_encode(raw);
        assert!(!encoded.contains(' '));
        assert_eq!(percent_decode(&encoded), raw);
    }

    #[test]
    fn range_parsing() {
        assert_eq!(parse_range(Some("bytes=0-99"), 1000), Some((0, 99)));
        assert_eq!(parse_range(Some("bytes=500-"), 1000), Some((500, 999)));
        assert_eq!(parse_range(Some("bytes=900-2000"), 1000), Some((900, 999)));
        assert_eq!(parse_range(Some("bytes=1000-"), 1000), None); // 越界
        assert_eq!(parse_range(None, 1000), None);
        assert_eq!(parse_range(Some("malformed"), 1000), None);
        // 后缀式、多段和反向 Range 都不支持；handler 会回答 416，不能退化成全量 200。
        assert_eq!(parse_range(Some("bytes=-500"), 1000), None);
        assert_eq!(parse_range(Some("bytes=0-9,20-29"), 1000), None);
        assert_eq!(parse_range(Some("bytes=500-100"), 1000), None);
    }

    fn request(path: &Path, range: Option<&str>) -> HttpRequest<Vec<u8>> {
        let mut builder = HttpRequest::builder().uri(format!(
            "http://preview./media?p={}",
            percent_encode(&path.to_string_lossy())
        ));
        if let Some(value) = range {
            builder = builder.header("range", value);
        }
        builder.body(Vec::new()).unwrap()
    }

    #[test]
    fn handle_bounds_full_and_range_responses() {
        let path = std::env::temp_dir().join(format!("sp_preview_{}.png", std::process::id()));
        std::fs::write(&path, b"0123456789").unwrap();

        let full = handle(request(&path, None));
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.body().as_ref(), b"0123456789");

        let partial = handle(request(&path, Some("bytes=2-5")));
        assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(partial.body().as_ref(), b"2345");
        assert_eq!(partial.headers()[CONTENT_RANGE], "bytes 2-5/10");

        let invalid = handle(request(&path, Some("bytes=-5")));
        assert_eq!(invalid.status(), StatusCode::RANGE_NOT_SATISFIABLE);

        std::fs::File::create(&path)
            .unwrap()
            .set_len(MAX_WINDOW + 1)
            .unwrap();
        assert_eq!(
            handle(request(&path, None)).status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        let _ = std::fs::remove_file(path);
    }

    /// 回归：单次响应封顶在 MAX_WINDOW——开区间与显式大 end 都不得展开成
    /// 整个大文件的一次性分配（GB 级输入 OOM → panic=abort 直接崩进程）。
    #[test]
    fn range_parsing_caps_single_response_window() {
        let gb = 2u64 * 1024 * 1024 * 1024;
        let cap = 8 * 1024 * 1024;
        assert_eq!(parse_range(Some("bytes=0-"), gb), Some((0, cap - 1)));
        assert_eq!(
            parse_range(Some(&format!("bytes=100-{gb}")), gb),
            Some((100, 100 + cap - 1))
        );
        // 窗口不干扰正常小请求
        assert_eq!(parse_range(Some("bytes=0-99"), gb), Some((0, 99)));
    }

    #[test]
    fn mime_by_extension() {
        assert_eq!(mime_for(Path::new("a.webm")), Some("video/webm"));
        assert_eq!(mime_for(Path::new("B.PNG")), Some("image/png"));
        // 回归：apng 曾只在 wasm 表里，桌面协议侧漏了 → 输入预览 404
        assert_eq!(mime_for(Path::new("a.apng")), Some("image/png"));
        assert_eq!(mime_for(Path::new("c.txt")), None);
    }

    #[test]
    fn media_url_format() {
        let url = media_url(Path::new(r"D:\x\y.png"));
        assert!(url.starts_with(URL_PREFIX));
        let url_cn = media_url(Path::new(r"D:\目 录\a.png"));
        assert!(url_cn.contains("%E7%9B%AE")); // 目
        assert!(url_cn.contains("%20"));
        assert_eq!(
            percent_decode(url_cn.rsplit("?p=").next().unwrap()),
            r"D:\目 录\a.png"
        );
    }
}
