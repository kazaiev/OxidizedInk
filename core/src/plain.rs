//! Plain-text and Markdown books, set as XHTML for the EPUB reading code:
//! a string per chapter, a table of contents from the headings, and a title
//! and author from the text where it gives them.
//!
//! Markdown is the common subset: ATX and underlined headings, paragraphs,
//! emphasis, code, block quotes, lists, tables, rules, links and front matter
//! (`title:`, `author:`, `lang:`). Pictures show their description, as the
//! card holds the book alone. Plain text is joined back into paragraphs,
//! with "Chapter …" lines as headings, and Project Gutenberg's `Title:`,
//! `Author:` and `Language:` lines as the metadata; its header and license
//! around the book are left out.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A chapter is closed at a block boundary once its XHTML is this long, so
/// a long text without headings still pages quickly.
const CHAPTER_BYTES: usize = 48 * 1024;

/// A converted book.
#[derive(Debug, Default)]
pub struct Converted {
    pub title: String,
    pub author: String,
    pub language: String,
    /// Each chapter's XHTML.
    pub chapters: Vec<String>,
    /// `(title, chapter, id of its heading, depth)`.
    pub toc: Vec<(String, usize, String, u8)>,
}

/// Reads a text file's bytes: UTF-8, or else Latin-1, as older text files
/// often are.
pub fn decode(data: &[u8]) -> String {
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    match core::str::from_utf8(data) {
        Ok(s) => s.replace("\r\n", "\n").replace('\r', "\n"),
        Err(_) => data.iter().map(|&b| b as char).collect::<String>().replace("\r\n", "\n").replace('\r', "\n"),
    }
}

/// A title from a file's name: "pride_and-prejudice.md" → "pride and prejudice".
fn title_from_name(name: &str) -> String {
    let stem = name.rsplit('/').next().unwrap_or(name);
    let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
    stem.replace(['_', '-'], " ").trim().to_string()
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// A heading's id, for links to it: lower case, words joined by hyphens.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if (c == ' ' || c == '-') && !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    String::from(out.trim_end_matches('-'))
}

/// A block of the book as it's set.
enum Block {
    Heading(u8, String),
    /// Other XHTML, such as a paragraph or a list.
    Html(String),
}

/// Gathers blocks into chapters: a new one at each heading of `split`
/// level or above, and when one grows past `CHAPTER_BYTES`.
fn assemble(blocks: Vec<Block>, mut out: Converted) -> Converted {
    let mut counts = [0usize; 7];
    for b in &blocks {
        if let Block::Heading(l, _) = b {
            counts[*l as usize] += 1;
        }
    }
    // Split at the shallowest level that heads more than one part.
    let split = (1..=6u8).find(|&l| counts[l as usize] > 1).unwrap_or(0);
    let top = (1..=6u8).find(|&l| counts[l as usize] > 0).unwrap_or(1);
    let mut chapter = String::new();
    let mut ids: Vec<(String, usize)> = Vec::new();
    for b in blocks {
        let (html, heading) = match b {
            Block::Heading(level, text) => {
                let mut id = slug(&text);
                if id.is_empty() || ids.iter().any(|(i, _)| *i == id) {
                    id = format!("h{}", ids.len());
                }
                (format!("<h{level} id=\"{id}\">{}</h{level}>\n", inline(&text)), Some((level, text, id)))
            }
            Block::Html(html) => (html, None),
        };
        let starts = heading.as_ref().is_some_and(|(l, ..)| split > 0 && *l <= split);
        if !chapter.is_empty() && (starts || chapter.len() > CHAPTER_BYTES) {
            out.chapters.push(core::mem::take(&mut chapter));
        }
        if let Some((level, text, id)) = heading {
            let n = out.chapters.len();
            // Only chapter headings: the reader starts a page at each entry.
            if split == 0 || level <= split {
                out.toc.push((plain_text(&text), n, id.clone(), level.saturating_sub(top)));
            }
            ids.push((id, n));
        }
        chapter.push_str(&html);
    }
    if !chapter.is_empty() || out.chapters.is_empty() {
        out.chapters.push(chapter);
    }
    // Links to headings in other chapters name the chapter, as the reader
    // finds an id within a chapter.
    for (n, chapter) in out.chapters.iter_mut().enumerate() {
        for (id, at) in &ids {
            if *at != n {
                *chapter = chapter.replace(&format!("href=\"#{id}\""), &format!("href=\"{at}#{id}\""));
            }
        }
    }
    out.chapters = out.chapters.into_iter().map(|c| format!("<html><body>{c}</body></html>")).collect();
    out
}

/// Markdown's inline marks gone, for titles.
fn plain_text(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '*' | '_' | '`')).collect::<String>().trim().to_string()
}

/// Inline Markdown to XHTML: emphasis, code, links, pictures' descriptions,
/// escapes and hard line breaks.
fn inline(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    // Open emphasis: "**" or "*" (or with "_"), outermost first.
    let mut open: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '\\' if next.is_some_and(|n| n.is_ascii_punctuation()) => {
                out.push_str(&escape(&next.unwrap().to_string()));
                i += 2;
            }
            '\n' => {
                if out.ends_with("  ") || out.ends_with('\\') {
                    while out.ends_with(' ') || out.ends_with('\\') {
                        out.pop();
                    }
                    out.push_str("<br/>");
                } else {
                    out.push(' ');
                }
                i += 1;
            }
            '`' => {
                let end = chars[i + 1..].iter().position(|&x| x == '`').map(|p| i + 1 + p);
                match end {
                    Some(end) => {
                        out.push_str(&escape(&chars[i + 1..end].iter().collect::<String>()));
                        i = end + 1;
                    }
                    None => {
                        out.push('`');
                        i += 1;
                    }
                }
            }
            '!' | '[' if link_at(&chars, if c == '!' { i + 1 } else { i }).is_some() => {
                let (text, href, end) = link_at(&chars, if c == '!' { i + 1 } else { i }).unwrap_or_default();
                if c == '!' || href.is_empty() {
                    if !text.is_empty() {
                        out.push_str(&format!("<i>{}</i>", inline(&text)));
                    }
                } else {
                    out.push_str(&format!("<a href=\"{}\">{}</a>", escape(&href), inline(&text)));
                }
                i = end;
            }
            '*' | '_' => {
                let double = next == Some(c);
                let mark = match (c, double) {
                    ('*', true) => "**",
                    ('*', false) => "*",
                    ('_', true) => "__",
                    _ => "_",
                };
                let len = mark.len();
                let before = i.checked_sub(1).map(|p| chars[p]);
                let after = chars.get(i + len).copied();
                // Underscores inside a word are just underscores.
                let inside = c == '_' && before.is_some_and(char::is_alphanumeric) && after.is_some_and(char::is_alphanumeric);
                let can_close = open.last() == Some(&mark) && before.is_some_and(|b| !b.is_whitespace());
                let can_open = after.is_some_and(|a| !a.is_whitespace());
                if inside {
                    out.push_str(mark);
                } else if can_close {
                    open.pop();
                    out.push_str(if double { "</b>" } else { "</i>" });
                } else if can_open {
                    open.push(mark);
                    out.push_str(if double { "<b>" } else { "<i>" });
                } else {
                    out.push_str(mark);
                }
                i += len;
            }
            c => {
                out.push_str(&escape(&c.to_string()));
                i += 1;
            }
        }
    }
    while let Some(mark) = open.pop() {
        out.push_str(if mark.len() == 2 { "</b>" } else { "</i>" });
    }
    out
}

/// A link (or a picture, after its "!") at `at`: `[text](href "title")`,
/// with its text, its href, and where it ends.
fn link_at(chars: &[char], at: usize) -> Option<(String, String, usize)> {
    if chars.get(at) != Some(&'[') {
        return None;
    }
    let mut depth = 0;
    let mut close = None;
    for (k, &c) in chars.iter().enumerate().skip(at) {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(k);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = chars[close + 2..].iter().position(|&c| c == ')').map(|p| close + 2 + p)?;
    let target: String = chars[close + 2..end].iter().collect();
    let href = target.split_whitespace().next().unwrap_or("").trim_matches(['<', '>']).to_string();
    Some((chars[at + 1..close].iter().collect(), href, end + 1))
}

/// A list item's marker: its indent, whether it's numbered, and where its
/// text starts.
fn list_marker(line: &str) -> Option<(usize, bool, usize)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[indent..];
    if let Some(r) = rest.strip_prefix(['-', '*', '+']) {
        if r.starts_with(' ') {
            return Some((indent, false, indent + 2));
        }
    }
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=9).contains(&digits) && rest[digits..].starts_with(['.', ')']) && rest[digits + 1..].starts_with(' ') {
        return Some((indent, true, indent + digits + 2));
    }
    None
}

/// A table's delimiter row: `|---|:---:|`.
fn table_delimiter(line: &str) -> bool {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    !t.is_empty()
        && t.split('|').all(|c| {
            let c = c.trim().trim_start_matches(':').trim_end_matches(':');
            !c.is_empty() && c.chars().all(|ch| ch == '-')
        })
}

/// A table row's cells, split at the pipes that aren't escaped.
fn table_cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').filter(|r| !r.ends_with('\\')).unwrap_or(t);
    let mut cells = Vec::from([String::new()]);
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cells.last_mut().unwrap().push('|');
                chars.next();
            }
            '|' => cells.push(String::new()),
            c => cells.last_mut().unwrap().push(c),
        }
    }
    cells.into_iter().map(|c| c.trim().to_string()).collect()
}

/// A table starting at line `start` (its header row): its XHTML and the
/// line after it.
fn table(lines: &[&str], start: usize) -> (String, usize) {
    let header = table_cells(lines[start]);
    let n = header.len();
    let row = |cells: Vec<String>, tag: &str| -> String {
        let cells = cells.into_iter().chain(core::iter::repeat(String::new())).take(n);
        let cells: String = cells.map(|c| format!("<{tag}>{}</{tag}>", inline(&c))).collect();
        format!("<tr>{cells}</tr>")
    };
    let mut out = format!("<table><thead>{}</thead><tbody>", row(header.clone(), "th"));
    let mut i = start + 2;
    while i < lines.len() && !lines[i].trim().is_empty() && lines[i].contains('|') {
        out.push_str(&row(table_cells(lines[i]), "td"));
        i += 1;
    }
    out.push_str("</tbody></table>\n");
    (out, i)
}

fn is_rule(line: &str) -> bool {
    let t: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3 && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*') || t.chars().all(|c| c == '_'))
}

fn atx_heading(line: &str) -> Option<(u8, String)> {
    let t = line.trim_start();
    let level = t.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&level) || !(t.len() == level || t[level..].starts_with(' ')) {
        return None;
    }
    let text = t[level..].trim().trim_end_matches('#').trim_end();
    Some((level as u8, text.to_string()))
}

/// Sets Markdown as blocks.
fn markdown_blocks(lines: &[&str], blocks: &mut Vec<Block>) {
    let mut para: Vec<&str> = Vec::new();
    let flush = |para: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if !para.is_empty() {
            blocks.push(Block::Html(format!("<p>{}</p>\n", inline(&para.join("\n")))));
            para.clear();
        }
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(&mut para, blocks);
            i += 1;
            continue;
        }
        // Fenced code.
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush(&mut para, blocks);
            let fence = &trimmed[..3];
            let end = lines[i + 1..].iter().position(|l| l.trim().starts_with(fence)).map_or(lines.len(), |p| i + 1 + p);
            blocks.push(Block::Html(format!("<pre>{}</pre>\n", escape(&lines[i + 1..end].join("\n")))));
            i = end + 1;
            continue;
        }
        // Indented code, outside a paragraph.
        if para.is_empty() && line.starts_with("    ") {
            let end = lines[i..].iter().position(|l| !l.starts_with("    ") && !l.trim().is_empty()).map_or(lines.len(), |p| i + p);
            let code: Vec<&str> = lines[i..end].iter().map(|l| l.get(4..).unwrap_or("")).collect();
            blocks.push(Block::Html(format!("<pre>{}</pre>\n", escape(code.join("\n").trim_end()))));
            i = end;
            continue;
        }
        if let Some((level, text)) = atx_heading(line) {
            flush(&mut para, blocks);
            blocks.push(Block::Heading(level, text));
            i += 1;
            continue;
        }
        // An underlined heading: the paragraph so far is its text.
        if !para.is_empty() && (trimmed.chars().all(|c| c == '=') || trimmed.chars().all(|c| c == '-')) {
            let level = if trimmed.starts_with('=') { 1 } else { 2 };
            blocks.push(Block::Heading(level, para.join(" ")));
            para.clear();
            i += 1;
            continue;
        }
        if is_rule(line) {
            flush(&mut para, blocks);
            blocks.push(Block::Html(String::from("<hr/>\n")));
            i += 1;
            continue;
        }
        if trimmed.starts_with('>') {
            flush(&mut para, blocks);
            let end = lines[i..].iter().position(|l| !l.trim_start().starts_with('>')).map_or(lines.len(), |p| i + p);
            let inner: Vec<&str> = lines[i..end].iter().map(|l| {
                let t = l.trim_start().strip_prefix('>').unwrap_or(l);
                t.strip_prefix(' ').unwrap_or(t)
            }).collect();
            let mut quoted = Vec::new();
            markdown_blocks(&inner, &mut quoted);
            let body: String = quoted.into_iter().map(|b| match b {
                Block::Heading(l, t) => format!("<h{l}>{}</h{l}>", inline(&t)),
                Block::Html(h) => h,
            }).collect();
            blocks.push(Block::Html(format!("<blockquote>{body}</blockquote>\n")));
            i = end;
            continue;
        }
        if para.is_empty() && line.contains('|') && lines.get(i + 1).is_some_and(|l| table_delimiter(l)) {
            let (html, end) = table(lines, i);
            blocks.push(Block::Html(html));
            i = end;
            continue;
        }
        if para.is_empty() && list_marker(line).is_some() {
            let (html, end) = list(lines, i);
            blocks.push(Block::Html(html));
            i = end;
            continue;
        }
        para.push(line);
        i += 1;
    }
    flush(&mut para, blocks);
}

/// A list starting at line `start`, with lists nested by indentation:
/// its XHTML and the line after it.
fn list(lines: &[&str], start: usize) -> (String, usize) {
    let mut out = String::new();
    // Open lists: (indent, numbered).
    let mut stack: Vec<(usize, bool)> = Vec::new();
    let mut item: Option<String> = None;
    let mut i = start;
    let close_item = |item: &mut Option<String>, out: &mut String| {
        if let Some(text) = item.take() {
            out.push_str(&format!("<li>{}</li>", inline(text.trim())));
        }
    };
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            // A blank line ends the list unless an item follows.
            if lines.get(i + 1).is_some_and(|l| list_marker(l).is_some()) {
                i += 1;
                continue;
            }
            break;
        }
        match list_marker(line) {
            Some((indent, numbered, text_at)) => {
                close_item(&mut item, &mut out);
                // Deeper lists end, and so does one of the other kind here.
                while stack.last().is_some_and(|&(d, n)| d > indent || d == indent && n != numbered) {
                    let (_, n) = stack.pop().unwrap();
                    out.push_str(if n { "</ol>" } else { "</ul>" });
                }
                if stack.last().is_none_or(|&(d, _)| d < indent) {
                    out.push_str(if numbered { "<ol>" } else { "<ul>" });
                    stack.push((indent, numbered));
                }
                item = Some(line[text_at..].to_string());
            }
            None if line.starts_with(' ') || item.is_some() && !is_rule(line) && atx_heading(line).is_none() => {
                // A continued item.
                if let Some(text) = &mut item {
                    text.push('\n');
                    text.push_str(line.trim());
                }
            }
            None => break,
        }
        i += 1;
    }
    close_item(&mut item, &mut out);
    while let Some((_, n)) = stack.pop() {
        out.push_str(if n { "</ol>" } else { "</ul>" });
    }
    out.push('\n');
    (out, i)
}

/// A Markdown book. `name` is its file's name, for a title if it has none.
pub fn markdown(text: &str, name: &str) -> Converted {
    let mut lines: Vec<&str> = text.split('\n').collect();
    let mut out = Converted::default();
    // Front matter.
    if lines.first().is_some_and(|l| l.trim() == "---") {
        if let Some(end) = lines.iter().skip(1).position(|l| l.trim() == "---" || l.trim() == "...") {
            for l in &lines[1..end + 1] {
                let Some((k, v)) = l.split_once(':') else { continue };
                let v = v.trim().trim_matches(['"', '\'']).to_string();
                match k.trim().to_ascii_lowercase().as_str() {
                    "title" => out.title = v,
                    "author" => out.author = v,
                    "lang" | "language" => out.language = v,
                    _ => {}
                }
            }
            lines.drain(..end + 2);
        }
    }
    let expanded: Vec<String> = lines.iter().map(|l| l.replace('\t', "    ")).collect();
    let lines: Vec<&str> = expanded.iter().map(String::as_str).collect();
    let mut blocks = Vec::new();
    markdown_blocks(&lines, &mut blocks);
    if out.title.is_empty() {
        out.title = blocks
            .iter()
            .find_map(|b| match b {
                Block::Heading(1, t) => Some(plain_text(t)),
                _ => None,
            })
            .unwrap_or_else(|| title_from_name(name));
    }
    assemble(blocks, out)
}

/// Plain text's `_italics_`, as Project Gutenberg marks them, as XHTML:
/// an underscore opens before a word and closes after one.
fn underscores(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut open = false;
    for (i, &c) in chars.iter().enumerate() {
        let before = i.checked_sub(1).map(|p| chars[p]);
        let after = chars.get(i + 1).copied();
        let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
        if c == '_' && !open && !word(before) && after.is_some_and(|a| !a.is_whitespace() && a != '_') {
            // Only if it closes in this paragraph.
            if (i + 2..chars.len()).any(|k| chars[k] == '_' && !word(chars.get(k + 1).copied())) {
                out.push_str("<i>");
                open = true;
                continue;
            }
        }
        if c == '_' && open && !word(after) {
            out.push_str("</i>");
            open = false;
            continue;
        }
        out.push(c);
    }
    if open {
        out.push_str("</i>");
    }
    out
}

/// A heading line in plain text: "Chapter 1", "CHAPTER XII.", "Part Two",
/// "Розділ 3".
fn plain_heading(line: &str) -> bool {
    let words = ["chapter", "part", "book", "prologue", "epilogue", "розділ", "частина"];
    let lower = line.trim().to_lowercase();
    lower.chars().count() <= 60 && words.iter().any(|w| lower == *w || lower.starts_with(&format!("{w} ")))
}

/// A plain-text book. `name` is its file's name, for a title if it has none.
pub fn plain(text: &str, name: &str) -> Converted {
    let mut out = Converted::default();
    let mut lines: Vec<&str> = text.split('\n').map(str::trim_end).collect();
    // Project Gutenberg's header names the book.
    for l in lines.iter().take(60) {
        if let Some((k, v)) = l.split_once(':') {
            let v = v.trim().to_string();
            match k.trim() {
                "Title" if out.title.is_empty() => out.title = v,
                "Author" if out.author.is_empty() => out.author = v,
                "Language" if out.language.is_empty() => {
                    out.language = String::from(match v.to_lowercase().as_str() {
                        "english" => "en",
                        "ukrainian" => "uk",
                        _ => "",
                    })
                }
                _ => {}
            }
        }
    }
    if out.title.is_empty() {
        out.title = title_from_name(name);
    }
    // A Project Gutenberg book: just the book, between its markers.
    let marker = |l: &&str, word: &str| l.starts_with("*** ") && l.contains(word) && l.contains("PROJECT GUTENBERG");
    if let Some(end) = lines.iter().position(|l| marker(l, "END OF")) {
        lines.truncate(end);
    }
    if let Some(start) = lines.iter().position(|l| marker(l, "START OF")) {
        lines.drain(..=start);
    }
    // Paragraphs are separated by blank lines, unless there are hardly any,
    // when each line is one.
    let blank = lines.iter().filter(|l| l.is_empty()).count();
    let by_line = lines.len() > 20 && blank * 20 < lines.len();
    let mut blocks = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let flush = |para: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if para.is_empty() {
            return;
        }
        // A chapter line, maybe with the chapter's name on the next lines.
        let short = para.len() <= 3 && para.iter().all(|l| l.trim().chars().count() <= 60);
        if short && plain_heading(para[0]) {
            let text: Vec<&str> = para.iter().map(|l| l.trim()).collect();
            blocks.push(Block::Heading(2, text.join(" ")));
        } else {
            let text: Vec<&str> = para.iter().map(|l| l.trim()).collect();
            blocks.push(Block::Html(format!("<p>{}</p>\n", underscores(&escape(&text.join(" "))))));
        }
        para.clear();
    };
    for line in lines {
        if line.trim().is_empty() {
            flush(&mut para, &mut blocks);
        } else {
            para.push(line);
            if by_line {
                flush(&mut para, &mut blocks);
            }
        }
    }
    flush(&mut para, &mut blocks);
    assemble(blocks, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{parse_xhtml, BlockKind, ITALIC};
    use alloc::vec;

    #[test]
    fn markdown_blocks_and_inline() {
        let book = markdown(
            "---\ntitle: \"Notes\"\nauthor: Ann\n---\n# One\n\nSome *em* and **strong**, `a<b`, snake_case and \
             a [link](https://x.org) and ![a cat](cat.png).\nSame paragraph.  \nNew line.\n\n\
             > Quoted\n> text\n\n- a\n- b\n  - nested\n1. first\n2. second\n\n---\n\n    code here\n\n\
             ```\nfenced <b>\n```\n\n# Two\n\nUnder\n=====\n",
            "notes.md",
        );
        assert_eq!((book.title.as_str(), book.author.as_str()), ("Notes", "Ann"));
        // "Under" is underlined as a first-level heading too: three chapters.
        assert_eq!(book.chapters.len(), 3);
        let ch = parse_xhtml(&book.chapters[0], |_| None);
        let texts: Vec<&str> = ch.blocks.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(texts[0], "One");
        assert_eq!(
            texts[1],
            "Some em and strong, a<b, snake_case and a link and a cat. Same paragraph.\nNew line."
        );
        assert_eq!(ch.blocks[1].style_at(5), ITALIC);
        assert_eq!(texts[2..5], ["Quoted text", "• a", "• b"]);
        assert_eq!(texts[5..8], ["• nested", "1. first", "2. second"]);
        assert_eq!(ch.blocks[8].kind, BlockKind::Rule);
        assert_eq!((ch.blocks[9].kind, texts[9]), (BlockKind::Pre, "code here"));
        assert_eq!(texts[10], "fenced <b>");
        // A link to a website shows as text; only links inside the book follow.
        assert!(ch.links.is_empty());
        let two = parse_xhtml(&book.chapters[1], |_| None);
        assert_eq!(two.blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Two"]);
        let toc: Vec<_> = book.toc.iter().map(|(t, n, _, d)| (t.as_str(), *n, *d)).collect();
        assert_eq!(toc, [("One", 0, 0), ("Two", 1, 0), ("Under", 2, 0)]);
    }

    #[test]
    fn markdown_tables() {
        let book = markdown(
            "| Path | What |\n|---|:---:|\n| `core/` | the *reader* |\n| a \\| b | |\n| short |\n\nAfter.\n",
            "t.md",
        );
        let ch = parse_xhtml(&book.chapters[0], |_| None);
        let rows: Vec<Vec<&str>> = ch
            .blocks
            .iter()
            .filter(|b| matches!(b.kind, BlockKind::Row { .. }))
            .map(|b| b.cells.iter().map(|&(s, e)| &b.text[s as usize..e as usize]).collect())
            .collect();
        assert_eq!(rows, [vec!["Path", "What"], vec!["core/", "the reader"], vec!["a | b", ""], vec!["short", ""]]);
        assert_eq!(ch.blocks[0].kind, BlockKind::Row { table: 1, header: true });
        assert_eq!(ch.blocks.last().unwrap().text, "After.");
        assert!(!table_delimiter("| a | b |"));
        assert!(table_delimiter("--- | :-:"));
    }

    #[test]
    fn markdown_links_to_other_chapters() {
        let book = markdown("# A\n\nSee [B](#b-part).\n\n# B part\n\nText.\n", "x.md");
        assert!(book.chapters[0].contains("href=\"1#b-part\""));
        assert!(book.chapters[1].contains("id=\"b-part\""));
        assert_eq!(book.title, "A");
    }

    #[test]
    fn plain_text_paragraphs_and_chapters() {
        let text = "Title: A Tale\nAuthor: Someone\nLanguage: English\n\n*** START OF THE PROJECT GUTENBERG EBOOK A TALE ***\n\n\
                    Front matter.\n\nCHAPTER I\nThe Best\n\nIt was the best\nof times.\n\n\
                    It was the worst.\n\nChapter 2\n\nOn & on <again>.\n\n*** END OF THE PROJECT GUTENBERG EBOOK A TALE ***\n\nLicense.\n";
        let book = plain(text, "tale.txt");
        assert_eq!((book.title.as_str(), book.author.as_str(), book.language.as_str()), ("A Tale", "Someone", "en"));
        assert_eq!(book.chapters.len(), 3);
        let one = parse_xhtml(&book.chapters[1], |_| None);
        let texts: Vec<&str> = one.blocks.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, ["CHAPTER I The Best", "It was the best of times.", "It was the worst."]);
        let front = parse_xhtml(&book.chapters[0], |_| None);
        assert_eq!(front.blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Front matter."]);
        let two = parse_xhtml(&book.chapters[2], |_| None);
        assert_eq!(two.blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Chapter 2", "On & on <again>."]);
        assert_eq!(book.toc.len(), 2);
        // Without blank lines, each line is a paragraph; no Title: line, the
        // name gives the title.
        let lines: String = (0..30).map(|i| format!("Line {i}\n")).collect();
        let book = plain(&lines, "my_notes.txt");
        assert_eq!(book.title, "my notes");
        assert_eq!(parse_xhtml(&book.chapters[0], |_| None).blocks.len(), 30);
    }

    #[test]
    fn long_texts_are_split_and_latin1_is_read() {
        let para = "word ".repeat(200);
        let text: String = (0..200).map(|_| format!("{para}\n\n")).collect();
        let book = plain(&text, "long.txt");
        assert!(book.chapters.len() > 3);
        assert!(book.chapters.iter().all(|c| c.len() < CHAPTER_BYTES + 2048));
        assert_eq!(decode(b"caf\xe9\r\nok"), "café\nok");
        assert_eq!(underscores("I'm sure _I_ shan't, _very well_ so, snake_case, _open"), "I'm sure <i>I</i> shan't, <i>very well</i> so, snake_case, _open");
        assert_eq!(decode("\u{feff}héllo".as_bytes()), "héllo");
    }
}
