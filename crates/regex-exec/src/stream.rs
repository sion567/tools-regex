//! 大文件流式扫描（设计方案第五章 / M3）。
//!
//! mmap 映射 + rayon 按行边界对齐分块并行；逐行用 `regex::bytes::Regex` 匹配
//! （bytes API 不要求 UTF-8，坏字节行照扫，预览 lossy 展示）。
//! 只支持 `regex` 引擎语法（线性时间）——环视/反向引用在编译期明确拒绝，
//! 这正是 DFA 家族"永远不炸"的正确用法（设计方案 7.5 第 3 条）。

use std::fs;
use std::time::Instant;

use memmap2::Mmap;
use rayon::prelude::*;
use regex::bytes::Regex;

/// 在 slice 的 [from..) 内找下一个换行的相对偏移
fn find_nl(slice: &[u8], from: usize) -> Option<usize> {
    slice[from..].iter().position(|b| *b == b'\n').map(|o| o + from)
}

/// 预览条数上限（设计方案 5.3 上下文采样）
pub const PREVIEW_CAP: usize = 50;
/// 单条预览截断长度（字节）
const PREVIEW_MAX_BYTES: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub struct LineHit {
    /// 1-based 行号
    pub line_no: u64,
    /// 文件字节偏移
    pub offset: u64,
    /// 该行命中次数
    pub match_count: u32,
    pub preview: String,
}

#[derive(Debug, Clone)]
pub struct StreamReport {
    pub path: String,
    pub bytes: u64,
    pub elapsed_ms: f32,
    pub chunks: usize,
    pub matched_lines: u64,
    pub total_matches: u64,
    /// MB/s 吞吐
    pub throughput_mbs: f32,
    pub preview: Vec<LineHit>,
}

pub fn scan_file(path: &str, pattern: &str) -> Result<StreamReport, String> {
    let t0 = Instant::now();
    let re = Regex::new(pattern)
        .map_err(|e| format!("大文件扫描仅支持 regex 引擎语法（线性时间）：{e}"))?;
    let file = fs::File::open(path).map_err(|e| format!("打开失败：{e}"))?;
    let len = file.metadata().map_err(|e| format!("取大小失败：{e}"))?.len();
    if len == 0 {
        return Ok(StreamReport {
            path: path.into(),
            bytes: 0,
            elapsed_ms: 0.0,
            chunks: 0,
            matched_lines: 0,
            total_matches: 0,
            throughput_mbs: 0.0,
            preview: vec![],
        });
    }
    let mmap = unsafe { Mmap::map(&file).map_err(|e| format!("mmap 失败：{e}"))? };

    // 分块：行边界对齐（块尾推进到下一个 \n 之后），顺带统计块前行号基数
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 16);
    let mut bounds: Vec<(u64, u64)> = Vec::new();
    let mut line_base: Vec<u64> = Vec::new();
    let ideal = len / workers as u64 + 1;
    let mut start = 0u64;
    let mut nl_seen = 0u64;
    while start < len {
        let mut end = (start + ideal).min(len);
        if end < len {
            // 推进到下一个换行之后
            let hay = &mmap[start as usize..];
            if let Some(o) = find_nl(hay, (end - start) as usize) {
                end = start + o as u64 + 1;
            } else {
                end = len;
            }
        }
        bounds.push((start, end));
        line_base.push(nl_seen);
        // 统计块内换行数（供下一块行号基数）
        nl_seen += mmap[start as usize..end as usize]
            .iter()
            .filter(|b| **b == b'\n')
            .count() as u64;
        start = end;
    }

    let chunk_stats: Vec<ChunkStats> = bounds
        .par_iter()
        .zip(&line_base)
        .map(|((s, e), base)| scan_chunk(&re, &mmap, *s, *e, *base))
        .collect();

    let mut report = StreamReport {
        path: path.into(),
        bytes: len,
        elapsed_ms: 0.0,
        chunks: chunk_stats.len(),
        matched_lines: 0,
        total_matches: 0,
        throughput_mbs: 0.0,
        preview: vec![],
    };
    for cs in chunk_stats {
        report.matched_lines += cs.matched_lines;
        report.total_matches += cs.total_matches;
        let mut take = report.preview.len() < PREVIEW_CAP;
        for h in cs.hits {
            if take && report.preview.len() < PREVIEW_CAP {
                report.preview.push(h);
            } else {
                take = false;
            }
        }
    }
    report.elapsed_ms = t0.elapsed().as_secs_f32() * 1000.0;
    report.throughput_mbs = len as f32 / 1e6 / (report.elapsed_ms / 1000.0).max(0.0001);
    Ok(report)
}

struct ChunkStats {
    matched_lines: u64,
    total_matches: u64,
    hits: Vec<LineHit>,
}

fn scan_chunk(re: &Regex, mmap: &Mmap, s: u64, e: u64, line_base: u64) -> ChunkStats {
    let slice = match mmap.get(s as usize..e as usize) {
        Some(v) => v,
        None => return ChunkStats { matched_lines: 0, total_matches: 0, hits: vec![] },
    };
    let mut stats = ChunkStats { matched_lines: 0, total_matches: 0, hits: vec![] };
    let mut local_line = 0u64;
    let mut line_start = 0usize;
    for line in slice.split(|b| *b == b'\n') {
        let offset_in_chunk = line_start;
        line_start += line.len() + 1;
        let cnt = re.find_iter(line).count() as u32;
        if cnt == 0 {
            local_line += 1;
            continue;
        }
        stats.matched_lines += 1;
        stats.total_matches += cnt as u64;
        if stats.hits.len() < PREVIEW_CAP {
            // 行内容去 \r + 截断
            let mut line = line;
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            let cut = line.len().min(PREVIEW_MAX_BYTES);
            stats.hits.push(LineHit {
                line_no: line_base + local_line + 1, // 1-based
                offset: s + offset_in_chunk as u64,
                match_count: cnt,
                preview: String::from_utf8_lossy(&line[..cut]).into_owned(),
            });
        }
        local_line += 1;
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "tools-regex-stream-{tag}-{}.log",
            std::process::id()
        ));
        let _ = fs::remove_file(&p);
        p
    }

    #[test]
    fn small_file_scan_counts_and_line_numbers() {
        let path = temp_path("ok");
        let mut f = fs::File::create(&path).unwrap();
        for i in 1..=1000u32 {
            if i % 7 == 0 {
                writeln!(f, "2026-11-18 worker ERROR code=12345 seq={i}").unwrap();
            } else {
                writeln!(f, "2026-11-18 worker INFO seq={i} ok").unwrap();
            }
        }
        drop(f);
        let report = scan_file(path.to_str().unwrap(), r"ERROR code=(\d{5})").unwrap();
        assert!(report.chunks >= 1);
        assert_eq!(report.matched_lines, 1000 / 7); // 7,14,...,994 → 142 行
        assert_eq!(report.total_matches, 142);
        assert_eq!(report.preview.len(), PREVIEW_CAP.min(142));
        let first = &report.preview[0];
        assert_eq!(first.line_no, 7);
        assert!(first.preview.contains("ERROR code=12345"));
        assert!(report.throughput_mbs > 0.0);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn lookahead_pattern_rejected_with_clear_error() {
        let path = temp_path("reject");
        fs::write(&path, "abc\n").unwrap();
        let err = scan_file(path.to_str().unwrap(), r"a(?=b)").unwrap_err();
        assert!(err.contains("仅支持 regex 引擎"), "{err}");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn missing_file_errors() {
        assert!(scan_file("E:/definitely/not/here.log", "a").is_err());
    }

    #[test]
    fn empty_file_ok() {
        let path = temp_path("empty");
        fs::write(&path, "").unwrap();
        let r = scan_file(path.to_str().unwrap(), "a").unwrap();
        assert_eq!(r.bytes, 0);
        assert_eq!(r.matched_lines, 0);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn invalid_utf8_lines_still_scanned() {
        let path = temp_path("binary");
        let mut data: Vec<u8> = Vec::new();
        data.extend_from_slice(b"ok line 1\n");
        data.extend_from_slice(&[0xFF, 0xFE, b'E', b'R', b'R', b'O', b'R', b'\n']);
        data.extend_from_slice(b"ok line 3\n");
        fs::write(&path, &data).unwrap();
        let r = scan_file(path.to_str().unwrap(), "ERROR").unwrap();
        assert_eq!(r.matched_lines, 1);
        assert!(r.preview[0].preview.contains("ERROR"));
        let _ = fs::remove_file(&path);
    }
}
