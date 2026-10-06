//! 极小的 HTTP/1.1 收发：只认够这个服务用的那部分——
//! 请求行 + 头 + `Content-Length` 定长的 `text/plain` 请求体，响应一律写完就关连接（不做 keep-alive）。
//!
//! 刻意手写而不是引 web 框架：这个服务只有三个路由，页面是一段内联 HTML，
//! 而壳那边已经有 tokio（仅 `rt`/`time`/`sync`，没有网络特性）或纯线程模型，
//! 为了三个路由把 async runtime 引进每个平台的进程不划算。

use std::io::{self, Read};

use crate::error::Error;

/// 头部区（请求行 + 所有头）上限，挡住畸形或恶意的大头。
const MAX_HEADER_BYTES: usize = 8 * 1024;

/// body 上限的硬顶。单次提交另有 [`crate::Config::max_text_bytes`]，这里留出余量给页面以外的 POST。
const MAX_BODY_BYTES: usize = 16 * 1024;

/// 一次请求读超时。手机在弱网下打开页面可能慢，但不会为了一个页面把连接挂一分钟。
pub(crate) const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// 带上自定义头的令牌。
pub(crate) const TOKEN_HEADER: &str = "x-qingjian-token";

/// 一个请求。只解析需要的字段，其余头按原样留着不解释。
#[derive(Debug)]
pub(crate) struct Request {
    pub method: String,
    /// 请求目标里 `?` 之前的路径。查询串（配对令牌）由页面自己在 JS 里读，服务端不解释它。
    pub path: String,
    pub(crate) headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// 取一个头的值（大小写不敏感）。重复的头取第一个。
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// 读一个完整请求：读到 `\r\n\r\n` 为止的头部区，再按 `Content-Length` 补齐 body。
pub(crate) fn read_request<R: Read>(reader: &mut R) -> Result<Request, Error> {
    let mut buf = Vec::with_capacity(1024);
    let head_end = loop {
        if let Some(at) = find_head_end(&buf) {
            break at;
        }
        if buf.len() > MAX_HEADER_BYTES {
            return Err(Error::TooLarge(MAX_HEADER_BYTES));
        }
        let mut chunk = [0u8; 1024];
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            return Err(Error::BadRequest("头部区不完整"));
        }
        buf.extend_from_slice(&chunk[..read]);
    };

    let head =
        std::str::from_utf8(&buf[..head_end]).map_err(|_| Error::BadRequest("头部不是 UTF-8"))?;
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split(' ');
    let method = request_line.next().unwrap_or_default().to_string();
    let target = request_line.next().unwrap_or_default().to_string();
    let version = request_line.next().unwrap_or_default();
    if method.is_empty() || target.is_empty() || !version.starts_with("HTTP/1.") {
        return Err(Error::BadRequest("请求行不合法"));
    }
    if !target.starts_with('/') {
        return Err(Error::BadRequest("只接受 origin-form 请求目标"));
    }

    let mut headers = Vec::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return Err(Error::BadRequest("头行缺少冒号"));
        };
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }

    if headers.iter().any(|(name, _)| name == "transfer-encoding") {
        return Err(Error::BadRequest("不支持分块传输"));
    }
    let length = match headers.iter().find(|(name, _)| name == "content-length") {
        Some((_, value)) => value
            .parse::<usize>()
            .map_err(|_| Error::BadRequest("Content-Length 不是数字"))?,
        None => 0,
    };
    if length > MAX_BODY_BYTES {
        return Err(Error::TooLarge(length));
    }

    // 头部区之后通常已经带上了 body 的一部分（小请求常常一次读完，大请求则要续读）。
    // 续读时只能要「还差的那点」：read_exact 是从头填的，把已读到的那段算进去会白等一次超时。
    let mut body = buf[head_end + 4..].to_vec();
    if body.len() > length {
        body.truncate(length);
    } else if body.len() < length {
        let mut rest = vec![0u8; length - body.len()];
        reader
            .read_exact(&mut rest)
            .map_err(|error| match error.kind() {
                io::ErrorKind::UnexpectedEof => Error::BadRequest("body 不完整"),
                _ => Error::Io(error),
            })?;
        body.append(&mut rest);
    }

    let path = target
        .split_once('?')
        .map_or(target.as_str(), |(path, _)| path)
        .to_string();
    Ok(Request {
        method,
        path,
        headers,
        body,
    })
}

/// 头部区结束位置（`\r\n\r\n` 里第一个 `\r` 的下标）。找不到返回 `None`。
fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// 写一个响应然后关连接。body 按 UTF-8 字节数算长度。
pub(crate) fn respond(stream: &mut impl io::Write, status: u16, body: &str) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\
         X-Content-Type-Options: nosniff\r\n\
         \r\n",
        reason = reason(status),
        len = body.len(),
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

/// 写一页 HTML（`GET /`）。和 [`respond`] 一样关连接，区别只是 `Content-Type`。
pub(crate) fn respond_html(stream: &mut impl io::Write, status: u16, body: &str) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\
         X-Content-Type-Options: nosniff\r\n\
         \r\n",
        reason = reason(status),
        len = body.len(),
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

/// 状态码的短语，只列这个服务用得到的几个。
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn parse(raw: &str) -> Result<Request, Error> {
        read_request(&mut Cursor::new(raw.as_bytes().to_vec()))
    }

    /// 每次只吐 `step` 字节的 reader，模拟 body 被拆到好几个 TCP 段里的情况。
    struct Trickle {
        data: Vec<u8>,
        step: usize,
    }

    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let take = self.step.min(buf.len()).min(self.data.len());
            buf[..take].copy_from_slice(&self.data[..take]);
            self.data.drain(..take);
            Ok(take)
        }
    }

    #[test]
    fn parses_a_post_with_body() {
        let request = parse("POST /commit HTTP/1.1\r\nHost: 192.168.1.2:23333\r\nX-Qingjian-Token: abc\r\nContent-Length: 12\r\n\r\n你好世界").unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/commit");
        assert_eq!(request.header("x-qingjian-token"), Some("abc"));
        assert_eq!(request.header("HOST"), Some("192.168.1.2:23333"));
        assert_eq!(String::from_utf8(request.body).unwrap(), "你好世界");
    }

    #[test]
    fn parses_a_get_without_body() {
        let request = parse("GET /status HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        assert_eq!(request.path, "/status");
        assert!(request.body.is_empty());
    }

    #[test]
    fn strips_the_query_string_from_the_path() {
        // 配对令牌在 ?k= 上，路由只看 ? 之前的部分。
        let request = parse("GET /?k=deadbeef HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        assert_eq!(request.path, "/");
        assert_eq!(
            parse("GET /status?x=1 HTTP/1.1\r\nHost: x\r\n\r\n")
                .unwrap()
                .path,
            "/status"
        );
    }

    #[test]
    fn rejects_a_malformed_request_line() {
        assert!(matches!(
            parse("GARBAGE\r\n\r\n"),
            Err(Error::BadRequest(_))
        ));
        assert!(matches!(parse("GET\r\n\r\n"), Err(Error::BadRequest(_))));
    }

    #[test]
    fn rejects_absolute_form_targets() {
        assert!(matches!(
            parse("GET http://evil/ HTTP/1.1\r\nHost: x\r\n\r\n"),
            Err(Error::BadRequest(_))
        ));
    }

    #[test]
    fn rejects_chunked_bodies() {
        let raw = "POST /commit HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert!(matches!(parse(raw), Err(Error::BadRequest(_))));
    }

    #[test]
    fn rejects_a_truncated_body() {
        let raw = "POST /commit HTTP/1.1\r\nHost: x\r\nContent-Length: 10\r\n\r\n短";
        assert!(matches!(parse(raw), Err(Error::BadRequest(_))));
    }

    #[test]
    fn rejects_oversized_headers() {
        let mut raw = String::from("GET / HTTP/1.1\r\n");
        for i in 0..2000 {
            raw.push_str(&format!("X-Pad-{i}: {}\r\n", "p".repeat(16)));
        }
        raw.push_str("\r\n");
        assert!(matches!(parse(&raw), Err(Error::TooLarge(_))));
    }

    #[test]
    fn reads_a_body_that_arrives_with_the_headers() {
        // 粘包：头部与 body 在同一个 read 里回来。
        let request =
            parse("POST /commit HTTP/1.1\r\nHost: x\r\nContent-Length: 3\r\n\r\nabc").unwrap();
        assert_eq!(request.body, b"abc");
    }

    #[test]
    fn reads_a_body_split_across_many_reads() {
        // body 与头部一次读完（buf 里已有）+ 后续再补，两段拼起来必须正好是 Content-Length。
        let raw = "POST /commit HTTP/1.1\r\nHost: x\r\nContent-Length: 12\r\n\r\n你好世界";
        let mut reader = Trickle {
            data: raw.as_bytes().to_vec(),
            step: 64,
        };
        let request = read_request(&mut reader).unwrap();
        assert_eq!(String::from_utf8(request.body).unwrap(), "你好世界");
        assert!(reader.data.is_empty(), "多读或漏读都会在这里露出来");
    }

    #[test]
    fn ignores_a_body_longer_than_content_length() {
        // 客户端把两个请求粘在一起时，多出来的字节属于下一个请求，这里丢掉。
        let raw = "POST /commit HTTP/1.1\r\nHost: x\r\nContent-Length: 3\r\n\r\nabcGET / HTTP/1.1\r\n\r\n";
        let request = parse(raw).unwrap();
        assert_eq!(request.body, b"abc");
    }

    #[test]
    fn writes_a_response_with_a_content_length() {
        let mut out = Vec::new();
        respond(&mut out, 409, "没有可输入的窗口").unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("HTTP/1.1 409 Conflict\r\n"));
        assert!(text.contains("Content-Length: 24\r\n"));
        assert!(text.contains("Cache-Control: no-store\r\n"));
        assert!(text.ends_with("没有可输入的窗口"));
    }

    #[test]
    fn writes_html_with_the_right_content_type() {
        let mut out = Vec::new();
        respond_html(&mut out, 200, "<p>hi</p>").unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Content-Type: text/html; charset=utf-8\r\n"));
        assert!(text.contains("Content-Length: 9\r\n"));
    }
}
