//! `cargo run --release --example server [端口]`：本地预览服务。
//!
//! 浏览器打开 http://127.0.0.1:8737 ，左边写 LaTeX，右边实时出图。
//! 零依赖：HTTP 自己解析，SVG 由 `mathengine::svg` 生成，渲染的是真引擎，不是前端模拟。

use mathengine::font::{load_latin_modern_math, MathFont};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Instant;

const PAGE: &str = r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8">
<title>mathengine 预览</title>
<style>
  :root { color-scheme: light; }
  * { box-sizing: border-box; }
  body { margin:0; font:15px/1.6 system-ui,-apple-system,"Noto Sans CJK SC",sans-serif;
         background:#faf9f7; color:#1d1b19; display:flex; height:100vh; }
  .pane { width:50%; padding:24px; display:flex; flex-direction:column; gap:12px; }
  .pane + .pane { border-left:1px solid #e5e1dc; background:#fff; }
  textarea { flex:1; resize:none; padding:16px; font:14px/1.7 ui-monospace,monospace;
             border:1px solid #dcd7d0; border-radius:10px; background:#fff; outline:none; }
  textarea:focus { border-color:#b8a88a; box-shadow:0 0 0 3px #b8a88a22; }
  .out { flex:1; display:flex; align-items:center; justify-content:center;
         overflow:auto; border:1px dashed #e5e1dc; border-radius:10px; padding:16px; }
  .out svg { max-width:100%; height:auto; }
  .bar { display:flex; gap:8px; flex-wrap:wrap; align-items:center; }
  button { font:inherit; font-size:13px; padding:5px 10px; border-radius:8px;
           border:1px solid #dcd7d0; background:#fff; cursor:pointer; }
  button:hover { background:#f3efe9; }
  .meta { font:13px ui-monospace,monospace; color:#6b6560; }
  .err { color:#b3261e; font:13px ui-monospace,monospace; white-space:pre-wrap; }
  .hint { font-size:13px; color:#8a847e; }
</style></head>
<body>
<div class="pane">
  <div class="bar">
    <strong>LaTeX</strong>
    <span class="hint">改写即时预览</span>
  </div>
  <textarea id="src" spellcheck="false">\frac{-b\pm\sqrt{b^2-4ac}}{2a}</textarea>
  <div class="bar">
    <button data-x="\frac{a}{b}">分式</button>
    <button data-x="\sqrt[3]{x+1}">根式</button>
    <button data-x="\sum_{i=1}^{n} i^2">求和</button>
    <button data-x="\int_0^1 x^2\,dx">积分</button>
    <button data-x="\begin{pmatrix} a & b \\ c & d \end{pmatrix}">矩阵</button>
    <button data-x="\binom{n}{k}">二项式</button>
    <button data-x="\lim_{x\to 0}\frac{\sin x}{x}=1">极限</button>
    <button data-x="\hat{a}+\bar{b}+\vec{v}">重音</button>
    <button data-x="\left[\frac{a}{b}\right]^2">自适应括号</button>
  </div>
</div>
<div class="pane">
  <div class="bar"><strong>渲染结果</strong><span class="meta" id="meta"></span></div>
  <div class="out" id="out"></div>
  <div class="err" id="err"></div>
</div>
<script>
const src = document.getElementById('src');
const out = document.getElementById('out');
const meta = document.getElementById('meta');
const err = document.getElementById('err');
let timer = null;

async function render() {
  const tex = src.value;
  try {
    const t0 = performance.now();
    const r = await fetch('/render?tex=' + encodeURIComponent(tex));
    const body = await r.text();
    const ms = performance.now() - t0;
    if (!r.ok) {
      err.textContent = body;
      meta.textContent = '';
      return;
    }
    err.textContent = '';
    out.innerHTML = body;
    const svg = out.querySelector('svg');
    meta.textContent = (svg ? svg.getAttribute('width') + '×' + svg.getAttribute('height') + '  ' : '')
      + r.headers.get('X-Glyphs') + ' 字形  '
      + r.headers.get('X-Micros') + ' µs  '
      + '往返 ' + ms.toFixed(0) + ' ms'
      + (r.headers.get('X-Missing') !== '0' ? '  缺字 ' + r.headers.get('X-Missing') : '');
    const uns = r.headers.get('X-Unsupported-List');
    if (uns) {
      err.textContent = '这些命令没被支持，已被丢弃：' + uns.split(',').map(c => '\\' + c).join(' ');
    }
  } catch (e) {
    err.textContent = String(e);
  }
}

function schedule() { clearTimeout(timer); timer = setTimeout(render, 180); }
src.addEventListener('input', schedule);
document.querySelectorAll('button[data-x]').forEach(b => {
  b.addEventListener('click', () => { src.value = b.dataset.x; render(); });
});
render();
</script>
</body></html>"#;

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(path: &str, key: &str) -> Option<String> {
    let q = path.split_once('?')?.1;
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            return Some(url_decode(v));
        }
    }
    None
}

fn respond(stream: &mut TcpStream, status: &str, body: &[u8], extra: &str) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn handle(mut stream: TcpStream, font: &MathFont) {
    let mut buf = [0u8; 8192];
    let n = match stream.read(&mut buf) {
        Ok(0) | Err(_) => return,
        Ok(n) => n,
    };
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/");

    if path == "/" || path.starts_with("/index") {
        respond(
            &mut stream,
            "200 OK",
            PAGE.as_bytes(),
            "Content-Type: text/html; charset=utf-8\r\n",
        );
        return;
    }

    let tex = query_param(path, "tex").unwrap_or_default();
    let t0 = Instant::now();
    match mathengine::svg::render(font, &tex, 56.0) {
        Ok(r) => {
            let us = t0.elapsed().as_micros();
            respond(
                &mut stream,
                "200 OK",
                r.svg.as_bytes(),
                &format!(
                    "Content-Type: image/svg+xml; charset=utf-8\r\nX-Glyphs: {}\r\nX-Missing: {}\r\nX-Unsupported: {}\r\nX-Unsupported-List: {}\r\nX-Micros: {}\r\nX-Size: {:.0}x{:.0}\r\n",
                    r.glyphs, r.missing, r.unsupported.len(), r.unsupported.join(","), us, r.width, r.height
                ),
            );
        }
        Err(e) => respond(
            &mut stream,
            "400 Bad Request",
            format!("解析失败：{e}").as_bytes(),
            "Content-Type: text/plain; charset=utf-8\r\n",
        ),
    }
}

fn main() {
    let port: u16 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(8737);
    let data = load_latin_modern_math(mathengine::LM_MATH_OTF).expect("字体文件应存在");
    let bytes: &'static [u8] = Box::leak(data.into_boxed_slice());
    let font = MathFont::from_bytes(bytes).expect("字体应能解析");

    let listener = TcpListener::bind(("127.0.0.1", port)).expect("端口被占用？");
    println!("预览已启动：http://127.0.0.1:{port}");
    for stream in listener.incoming() {
        match stream {
            Ok(s) => handle(s, &font),
            Err(_) => continue,
        }
    }
}
