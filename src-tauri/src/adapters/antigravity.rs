//! Google Antigravity CLI（`agy`）会话：`~/.gemini/antigravity-cli/`
//!
//! 布局（agy 2026-09 版本地实测）：
//! - `conversations/<uuid>.db`：一个对话一个 SQLite，正文全是 protobuf blob
//! - `conversation_summaries.db`：所有对话的元数据，明文列 `title` / `preview` /
//!   `workspace_uris`（file:// URI 的 JSON 数组）/ `last_modified_time` / `parent_conversation_id`
//! - `brain/<uuid>/`、`annotations/<uuid>.pbtxt`：对话自己的附属文件
//!
//! **用量**在每个对话库的 `gen_metadata` 表，一行一次模型调用。blob 是 Codeium 系的
//! `ModelUsageStats`，没有 .proto，按字段号对照实测数据确认：
//! `data.1.4` = { 1 模型枚举（1318 ↔ `MODEL_PLACEHOLDER_M318`）, 2 input, 3 output,
//! 4 cache_write, 5 cache_read, 6 供应商, 9 思考 token, 10 回复 token }，模型名在 `data.1.19`。
//!
//! 口径由数据定，不照搬别家：
//! - **output 已含思考**：54 次调用全部满足 `f3 = f9 + f10`（如 341 = 251 + 90）
//! - **cache_read 独立于 input**（Anthropic 式，和 Z Code 相反）：缓存未命中那次的 input
//!   接得上前一次的 input + cache_read（3,444 + 44,849 = 48,293 → 下一次未命中 48,682），
//!   上下文只是在涨，没有重叠。所以四项直接相加就是总量
//! - 两行全零、没有模型名的调用（被取消的）不计次数
//!
//! 只读，也不接删除：`agy` 没有删除命令，而删对话文件会在 `conversation_summaries.db` 里
//! 留下死记录——那是 agy 的库，我们不写。恢复用 `agy --conversation <id>`（见 `resume.rs`）。

use super::opencode::{open, summarize, Row};
use super::{dir_size, storage_absent, system_time_ms, HarnessStorage, SessionSummary, TokenUsage};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn agy_home() -> Option<PathBuf> {
    // 本机这是个指向安装目录的软链接，is_dir 会跟随
    let p = super::home_dir()?.join(".gemini").join("antigravity-cli");
    p.is_dir().then_some(p)
}

/* ── protobuf：只需要按字段号取值，手写一个最小解析 ── */

enum Val<'a> {
    Int(u64),
    Bytes(&'a [u8]),
}

fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let c = *b.get(*i)?;
        *i += 1;
        v |= u64::from(c & 0x7f) << shift;
        if c < 0x80 {
            return Some(v);
        }
    }
    None
}

/// 一层字段；遇到解析不了的就返回 None（blob 不是这个结构）
fn fields(b: &[u8]) -> Option<Vec<(u64, Val<'_>)>> {
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        let key = varint(b, &mut i)?;
        let val = match key & 7 {
            0 => Val::Int(varint(b, &mut i)?),
            1 => {
                i = i.checked_add(8).filter(|&e| e <= b.len())?;
                continue;
            }
            5 => {
                i = i.checked_add(4).filter(|&e| e <= b.len())?;
                continue;
            }
            2 => {
                let n = usize::try_from(varint(b, &mut i)?).ok()?;
                let end = i.checked_add(n).filter(|&e| e <= b.len())?;
                let s = &b[i..end];
                i = end;
                Val::Bytes(s)
            }
            _ => return None,
        };
        out.push((key >> 3, val));
    }
    Some(out)
}

fn sub(b: &[u8], field: u64) -> Option<&[u8]> {
    fields(b)?.into_iter().find_map(|(f, v)| match v {
        Val::Bytes(s) if f == field => Some(s),
        _ => None,
    })
}

/// 一行 `gen_metadata.data` → (用量, 模型名)
fn call_usage(blob: &[u8]) -> Option<(TokenUsage, String)> {
    let meta = sub(blob, 1)?;
    let model = sub(meta, 19).and_then(|s| std::str::from_utf8(s).ok()).unwrap_or("").to_string();
    let mut n: HashMap<u64, u64> = HashMap::new();
    for (f, v) in fields(sub(meta, 4).unwrap_or(&[]))? {
        if let Val::Int(x) = v {
            n.insert(f, x);
        }
    }
    let get = |f| n.get(&f).copied().unwrap_or(0);
    let (thinking, reply) = (get(9), get(10));
    // 实测 f3 = 思考 + 回复；若哪天改成 f3 只含回复，就把思考补进来
    let output = if thinking > 0 && get(3) == reply { reply + thinking } else { get(3) };
    let usage = TokenUsage {
        input: get(2),
        output,
        cache_write: get(4),
        cache_read: get(5),
        unsplit: 0,
        calls: 0,
    };
    Some((usage, model))
}

/// 一个对话库里所有调用的合计与最后用到的模型
fn conversation_usage(db: &Path) -> (TokenUsage, String) {
    let mut total = TokenUsage::default();
    let mut model = String::new();
    let Some(con) = open(db) else { return (total, model) };
    let Ok(mut stmt) = con.prepare("SELECT data FROM gen_metadata ORDER BY idx") else { return (total, model) };
    let Ok(rows) = stmt.query_map([], |r| r.get::<_, Vec<u8>>(0)) else { return (total, model) };
    for blob in rows.flatten() {
        let Some((mut u, m)) = call_usage(&blob) else { continue };
        if u.total() == 0 {
            continue; // 被取消的调用：全零、没有模型名
        }
        u.calls = 1;
        total.add(&u);
        if !m.is_empty() {
            model = m;
        }
    }
    (total, model)
}

/* ── 元数据 ── */

struct Meta {
    title: String,
    directory: String,
    parent: Option<String>,
    updated_ms: u64,
}

fn read_summaries(home: &Path) -> HashMap<String, Meta> {
    let mut out = HashMap::new();
    let Some(con) = open(&home.join("conversation_summaries.db")) else { return out };
    let Ok(mut stmt) = con.prepare(
        "SELECT conversation_id, title, preview, workspace_uris, CAST(last_modified_time AS TEXT), parent_conversation_id
         FROM conversation_summaries",
    ) else {
        return out;
    };
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            r.get::<_, Option<String>>(4)?.unwrap_or_default(),
            r.get::<_, Option<String>>(5)?.unwrap_or_default(),
        ))
    });
    let Ok(rows) = rows else { return out };
    for (id, title, preview, uris, modified, parent) in rows.flatten() {
        // 标题还没生成的对话，preview 是第一句用户输入
        let title = if title.trim().is_empty() { preview } else { title };
        let directory = serde_json::from_str::<Vec<String>>(&uris)
            .ok()
            .and_then(|v| v.into_iter().next())
            .map(|u| file_uri_to_path(&u))
            .unwrap_or_default();
        out.insert(
            id,
            Meta {
                title,
                directory,
                parent: Some(parent).filter(|p| !p.is_empty()),
                updated_ms: parse_time_ms(&modified).unwrap_or(0),
            },
        );
    }
    out
}

/// `file:///D:/%E6%96%87%E6%A1%A3` → `D:/文档`；`file:///home/x` → `/home/x`
fn file_uri_to_path(uri: &str) -> String {
    let rest = uri.strip_prefix("file://").unwrap_or(uri);
    let bytes = rest.as_bytes();
    let mut raw = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                raw.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        raw.push(bytes[i]);
        i += 1;
    }
    let path = String::from_utf8_lossy(&raw).to_string();
    // Windows 盘符前多出来的那个 `/`
    let b = path.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        path[1..].to_string()
    } else {
        path
    }
}

/// `2026-09-19 08:22:07.6228218+00:00`（也接受 `T` 分隔和 `Z`）→ Unix 毫秒
fn parse_time_ms(s: &str) -> Option<u64> {
    let s = s.trim();
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, sec) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let rest = s.get(19..).unwrap_or("");
    let (frac, tz) = match rest.find(['+', '-', 'Z']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let ms = frac.strip_prefix('.').map_or(0, |f| {
        let digits: String = f.chars().take(3).collect();
        format!("{digits:0<3}").parse::<i64>().unwrap_or(0)
    });
    let offset_min = match tz.as_bytes().first() {
        Some(b'+') | Some(b'-') => {
            let sign = if tz.starts_with('-') { -1 } else { 1 };
            let hh = tz.get(1..3)?.parse::<i64>().ok()?;
            let mm = tz.get(4..6).and_then(|m| m.parse::<i64>().ok()).unwrap_or(0);
            sign * (hh * 60 + mm)
        }
        _ => 0,
    };
    // 公历日期 → 自 1970-01-01 的天数（Howard Hinnant 的 days_from_civil）
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * ((mo + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + sec - offset_min * 60;
    u64::try_from(secs * 1000 + ms).ok()
}

/* ── 列表 ── */

/// 对话自己的文件：库（含 WAL）、brain 目录、批注
fn conversation_bytes(home: &Path, id: &str) -> u64 {
    let conv = home.join("conversations");
    let file = |p: PathBuf| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    ["db", "db-wal", "db-shm"].iter().map(|ext| file(conv.join(format!("{id}.{ext}")))).sum::<u64>()
        + file(home.join("annotations").join(format!("{id}.pbtxt")))
        + dir_size(&home.join("brain").join(id))
}

fn list(home: &Path) -> Result<Vec<SessionSummary>, String> {
    let Ok(entries) = std::fs::read_dir(home.join("conversations")) else { return Ok(vec![]) };
    let mut meta = read_summaries(home);
    let mut rows = vec![];
    for db in entries.flatten().map(|e| e.path()) {
        if db.extension().and_then(|e| e.to_str()) != Some("db") {
            continue;
        }
        let Some(id) = db.file_stem().and_then(|s| s.to_str()).map(String::from) else { continue };
        let (usage, model) = conversation_usage(&db);
        let m = meta.remove(&id);
        // 摘要库里没有这条时，用库文件自己的修改时间
        let mtime = ["db", "db-wal"]
            .iter()
            .filter_map(|ext| std::fs::metadata(db.with_extension(ext)).ok()?.modified().ok())
            .map(system_time_ms)
            .max()
            .unwrap_or(0);
        rows.push(Row {
            parent: m.as_ref().and_then(|m| m.parent.clone()),
            title: m.as_ref().map(|m| m.title.clone()).unwrap_or_default(),
            directory: m.as_ref().map(|m| m.directory.clone()).unwrap_or_default(),
            model: if model.is_empty() { "—".into() } else { model },
            updated_ms: m.as_ref().map(|m| m.updated_ms).filter(|&t| t > 0).unwrap_or(mtime),
            usage,
            id,
        });
    }
    let conv = home.join("conversations");
    summarize("antigravity", rows, &conv, |id, _| conversation_bytes(home, id))
}

pub fn list_sessions() -> Result<Vec<SessionSummary>, String> {
    match agy_home() {
        Some(home) => list(&home),
        None => Ok(vec![]),
    }
}

pub fn storage() -> HarnessStorage {
    let Some(home) = agy_home() else {
        return storage_absent("antigravity", "~/.gemini/antigravity-cli/");
    };
    let sessions = list(&home).unwrap_or_default();
    HarnessStorage {
        harness: "antigravity".into(),
        connected: true,
        sessions: sessions.len() as u32,
        session_bytes: sessions.iter().map(|s| s.size_bytes).sum(),
        root_bytes: dir_size(&home),
        root: "~/.gemini/antigravity-cli/".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint_bytes(mut v: u64) -> Vec<u8> {
        let mut out = vec![];
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return out;
            }
            out.push(b | 0x80);
        }
    }
    fn int(field: u64, v: u64) -> Vec<u8> {
        [varint_bytes(field << 3), varint_bytes(v)].concat()
    }
    fn msg(field: u64, body: &[u8]) -> Vec<u8> {
        [varint_bytes((field << 3) | 2), varint_bytes(body.len() as u64), body.to_vec()].concat()
    }

    /// 一行真实调用的形状：data.1.4 = 用量，data.1.19 = 模型名
    fn blob(input: u64, output: u64, cache_read: u64, thinking: u64, reply: u64) -> Vec<u8> {
        let usage = [int(1, 1318), int(2, input), int(3, output), int(5, cache_read), int(9, thinking), int(10, reply)].concat();
        let meta = [int(3, 1318), msg(4, &usage), msg(19, b"gemini-3.8-flash")].concat();
        [msg(2, b"xx"), msg(1, &meta)].concat()
    }

    /// 本机真实数值：output 已含思考，cache_read 与 input 分开记
    #[test]
    fn real_call_maps_to_non_overlapping_buckets() {
        let (u, model) = call_usage(&blob(3_444, 155, 44_849, 31, 124)).unwrap();
        assert_eq!((u.input, u.output, u.cache_read, u.cache_write), (3_444, 155, 44_849, 0));
        assert_eq!(u.total(), 3_444 + 155 + 44_849);
        assert_eq!(model, "gemini-3.8-flash");
    }

    #[test]
    fn thinking_is_added_only_if_output_left_it_out() {
        let (inside, _) = call_usage(&blob(100, 341, 0, 251, 90)).unwrap();
        assert_eq!(inside.output, 341);
        let (outside, _) = call_usage(&blob(100, 90, 0, 251, 90)).unwrap();
        assert_eq!(outside.output, 341);
    }

    #[test]
    fn garbage_blobs_are_rejected_not_panicking() {
        for b in [&[0xff, 0xff, 0xff][..], &[0x0a, 0x50, 0x01][..], &[][..]] {
            assert!(call_usage(b).is_none() || call_usage(b).unwrap().0.total() == 0);
        }
    }

    #[test]
    fn file_uris_decode_to_paths() {
        assert_eq!(file_uri_to_path("file:///D:/%E7%AC%94%E8%AE%B0/notes"), "D:/笔记/notes");
        assert_eq!(file_uri_to_path("file:///home/me/p%20q"), "/home/me/p q");
        assert_eq!(file_uri_to_path("file:///D:/100%"), "D:/100%");
    }

    #[test]
    fn times_parse_with_offsets_and_fractions() {
        // 2026-09-19 08:22:07.622 UTC
        assert_eq!(parse_time_ms("2026-09-19 08:22:07.6228218+00:00"), Some(1_789_806_127_622));
        assert_eq!(parse_time_ms("2026-09-19T16:22:07.622+08:00"), Some(1_789_806_127_622));
        assert_eq!(parse_time_ms("2026-09-19T08:22:07Z"), Some(1_789_806_127_000));
        assert_eq!(parse_time_ms("garbage"), None);
    }
}
