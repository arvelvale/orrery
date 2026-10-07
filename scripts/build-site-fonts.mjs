// 把落地页实际用到的字从完整字体里抽出来，生成 site/fonts/*.woff2。
//
// 为什么自托管：落地页承诺“不把访客交给任何第三方”，所以不用 Google Fonts 之类的 CDN；
// 为什么做子集：完整的思源黑体一套 17 MB，页面只用到几百个汉字，子集后约 200 KB。
// 改了页面文案（site/index.html 里出现了新字）就要重新跑一次，否则新字会回退到系统字体。
//
// 用法：
//   npm i --no-save subset-font
//   FONT_SRC=<放完整字体的目录> node scripts/build-site-fonts.mjs
//
// FONT_SRC 里需要这四个文件（均为 SIL OFL 1.1，可从 github.com/google/fonts 的 ofl/ 目录取得）：
//   Geist[wght].ttf  GeistMono[wght].ttf  NotoSansSC[wght].ttf  NotoSansJP[wght].ttf
// 许可证文本随字体放在 site/fonts/OFL-*.txt，更换字体时一并更新。
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import subsetFont from "subset-font";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const SRC = process.env.FONT_SRC;
if (!SRC) { console.error("set FONT_SRC to the folder holding the full .ttf files"); process.exit(1); }
const OUT = path.join(root, "site", "fonts");
fs.mkdirSync(OUT, { recursive: true });

const html = fs.readFileSync(path.join(root, "site", "index.html"), "utf8");
// 页面文字 + Intl 日期格式会用到的字 + 常见标点
const extra = "年月日0123456789,.:-/ ·—…「」『』（）、。！？：；";
const chars = new Set([...html, ...extra]);
const cjk = [...chars].filter((c) => c.charCodeAt(0) > 0x2e7f).join("");
const ascii = Array.from({ length: 95 }, (_, i) => String.fromCharCode(32 + i)).join("");
const latin = [...chars].filter((c) => c.charCodeAt(0) >= 0x20 && c.charCodeAt(0) <= 0x2e7f).join("") + ascii;

async function make(file, text, name) {
  const out = await subsetFont(fs.readFileSync(path.join(SRC, file)), text, { targetFormat: "woff2" });
  fs.writeFileSync(path.join(OUT, name), out);
  console.log(name.padEnd(16), `${(out.length / 1024).toFixed(1)} KB`.padStart(9), `${[...text].length} chars`);
}

await make("Geist[wght].ttf", latin, "geist.woff2");
await make("GeistMono[wght].ttf", latin, "geist-mono.woff2");
await make("NotoSansSC[wght].ttf", cjk, "noto-sc.woff2");
await make("NotoSansJP[wght].ttf", cjk, "noto-jp.woff2");
