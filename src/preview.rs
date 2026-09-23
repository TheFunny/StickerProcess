//! `preview://` 自定义协议（Phase D 双轨制的"大文件轨"）。
//!
//! WebView2 把名为 `preview` 的自定义协议映射为 `http://preview./…`，
//! 本模块提供 URL 构造与请求 handler：按扩展名给 MIME、支持 HTTP Range
//! （视频拖动必需）、按需读盘（配合 OS 页缓存，无需自建缓存层）。
//!
//! 安全性：URL 携带 percent-encoded 的本机路径，但该协议仅注册在本进程的
//! WebView 内，外部无法访问。

use dioxus::desktop::{Config, wry};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
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

    match parse_range(
        request.headers().get("range").and_then(|v| v.to_str().ok()),
        total,
    ) {
        Some((start, end)) => match read_slice(&path, start, end) {
            Some(data) => partial(mime, data, start, end, total),
            None => not_found(),
        },
        None => match std::fs::read(&path) {
            // Content-Length 用实际读到的字节数：metadata 与 read 之间文件
            // 增长会让头/体长度不一致
            Ok(data) => full(mime, data),
            Err(_) => not_found(),
        },
    }
}

type Body = Vec<u8>;

/// 单次 Range 响应窗口上限（开区间与超大闭区间一律适用）。预览协议就是为
/// "大文件轨"设计的，而 Chromium/WebView2 对 `<video>` 的首个请求常是
/// `bytes=0-`（开区间）——展开为整个文件一次分配，GB 级输入直接 OOM，
/// `panic=abort` 下就是进程崩溃。截成 8 MiB 窗口后按 206 返回（Content-Range
/// 如实标注），媒体元素按窗口续发 Range 续读，起播不受影响。
/// 残余：无 Range 头的 200 全量读仍整读进内存——wry 的自定义协议响应体只收
/// 完整字节、流式不可用；图片输入是 MB 级可接受，视频实测走 Range 路径。
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

fn not_found() -> HttpResponse<Cow<'static, [u8]>> {
    HttpResponse::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Cow::Borrowed(&b"not found"[..]))
        .unwrap()
}

/// 解析 `Range: bytes=start-end` / `bytes=start-`，返回闭区间 (start, end)。
/// 无/非法 Range 返回 None（走 200 全量）。
fn parse_range(header: Option<&str>, total: u64) -> Option<(u64, u64)> {
    let header = header?;
    let spec = header.strip_prefix("bytes=")?;
    let (start_s, end_s) = spec.split_once('-')?;
    let start: u64 = start_s.trim().parse().ok()?;
    if start >= total {
        return None;
    }
    let end = match end_s.trim().parse::<u64>() {
        Ok(end) => end.min(total - 1),
        Err(_) => total - 1, // open-ended "bytes=N-"
    };
    // 单次响应窗口：闭区间的显式大 end 与开区间一样受约束（见 MAX_WINDOW）
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
        // `\` 与 `:` 在白名单内保持原样，中文/空格才转义
        assert!(url.contains("media?p=D:\\x\\y.png"));
        let url_cn = media_url(Path::new(r"D:\目 录\a.png"));
        assert!(url_cn.contains("%E7%9B%AE")); // 目
        assert!(url_cn.contains("%20"));
        assert_eq!(
            percent_decode(url_cn.rsplit("?p=").next().unwrap()),
            r"D:\目 录\a.png"
        );
    }
}
