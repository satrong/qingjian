//! 手机上打开的那一页：一个多行输入框 + 发送按钮 + 一行状态。
//!
//! 整页内联（无外链、无框架），因为它从一个明文 HTTP 的局域网地址加载，且要求只有「一个 HTML 文件」，
//! 服务端不必再管静态资源路由。语音不在这里识别：切到手机自己的输入法、按它的麦克风，
//! 识别结果落进输入框，点发送即可——所以这一页不需要麦克风权限，也不必是 HTTPS。

/// `GET /` 的响应体。
pub const PAGE: &str = r##"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<title>清简 · 手机输入</title>
<style>
:root{color-scheme:light dark;--bg:#f6f6f7;--fg:#1c1c1e;--dim:#8a8a8e;--line:#d8d8dc;--accent:#2f6fed;--card:#fff}
@media(prefers-color-scheme:dark){:root{--bg:#151517;--fg:#f2f2f4;--dim:#98989e;--line:#333338;--accent:#5b8dff;--card:#1e1e21}}
*{box-sizing:border-box}
html,body{height:100%}
body{margin:0;background:var(--bg);color:var(--fg);font:17px/1.5 -apple-system,BlinkMacSystemFont,"PingFang SC","Noto Sans CJK SC",sans-serif;display:flex;flex-direction:column;padding:max(16px,env(safe-area-inset-top)) 16px max(16px,env(safe-area-inset-bottom))}
h1{font-size:15px;font-weight:600;margin:0 0 12px;color:var(--dim);letter-spacing:.02em}
textarea{flex:1;width:100%;resize:none;border:1px solid var(--line);border-radius:12px;background:var(--card);color:var(--fg);padding:14px;font:inherit;outline:none}
textarea:focus{border-color:var(--accent)}
.bar{display:flex;gap:10px;margin-top:12px}
button{flex:1;padding:13px 16px;border-radius:12px;border:1px solid var(--line);background:var(--card);color:var(--fg);font:600 17px/1 inherit;cursor:pointer}
button.primary{background:var(--accent);border-color:transparent;color:#fff}
button:disabled{opacity:.5}
#status{margin-top:12px;min-height:1.5em;font-size:15px;color:var(--dim);overflow-wrap:anywhere}
#status.ok{color:var(--accent)}
#status.bad{color:#e5484d}
@media(prefers-color-scheme:dark){#status.bad{color:#ff6369}}
</style>
</head>
<body>
<h1>清简 · 手机输入</h1>
<textarea id="text" placeholder="切到手机输入法，按麦克风说话…" autocapitalize="off" autocomplete="off" enterkeyhint="send"></textarea>
<div class="bar">
<button id="clear" type="button">清空</button>
<button id="send" type="button" class="primary">发送到电脑</button>
</div>
<div id="status"></div>
<script>
const TOKEN_KEY = "qj.token";
const $ = (id) => document.getElementById(id);
const status = $("status"), text = $("text"), send = $("send");

// 二维码把令牌放在 ?k= 上，存下来后立刻把地址栏洗掉，别让令牌留在浏览器历史里。
const fromQr = new URLSearchParams(location.search).get("k");
if (fromQr) {
  localStorage.setItem(TOKEN_KEY, fromQr);
  history.replaceState(null, "", location.pathname);
}
const token = localStorage.getItem(TOKEN_KEY) || "";
if (!token) say("未配对：请扫描电脑上的二维码", 0);

function say(message, code) {
  status.textContent = message + (code ? "（" + code + "）" : "");
  status.className = code === 200 ? "ok" : code ? "bad" : "";
}

async function submit() {
  const body = text.value.trim();
  if (!body) { say("先说点什么", 0); text.focus(); return; }
  if (!token) { say("未配对：请扫描电脑上的二维码", 0); return; }
  send.disabled = true;
  try {
    const response = await fetch("/commit", {
      method: "POST",
      headers: { "Content-Type": "text/plain;charset=utf-8", "X-Qingjian-Token": token },
      body: body,
    });
    const message = (await response.text()).trim();
    if (response.ok) { text.value = ""; say(message || "已上屏", 200); }
    else say(message || "提交失败", response.status);
  } catch (error) {
    say("连不上电脑：" + error.message, 0);
  } finally {
    send.disabled = false;
    text.focus();
  }
}

send.addEventListener("click", submit);
$("clear").addEventListener("click", () => { text.value = ""; text.focus(); });
// 回车发送、换行仍靠输入法自己的「换行」键
text.addEventListener("keydown", (event) => { if (event.key === "Enter" && !event.shiftKey && !event.isComposing) { event.preventDefault(); submit(); } });
text.focus();
</script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_is_self_contained() {
        // 不引任何外链：页面只能来自局域网地址上的这一个响应。
        assert!(!PAGE.contains("http://"));
        assert!(!PAGE.contains("https://"));
        assert!(!PAGE.contains("<link"));
        assert!(!PAGE.contains("src="));
    }

    #[test]
    fn page_sends_the_token_header() {
        assert!(PAGE.contains(r#""X-Qingjian-Token": token"#));
        assert!(PAGE.contains(r#"fetch("/commit""#));
    }

    #[test]
    fn page_strips_the_token_from_the_address_bar() {
        assert!(PAGE.contains("history.replaceState"));
    }
}
