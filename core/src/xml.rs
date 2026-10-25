//! A small, forgiving XML/XHTML tokenizer. It never fails: malformed markup
//! degrades to text, which is the right trade-off for real-world EPUBs.

use alloc::string::String;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Token<'a> {
    /// `<name attrs>`; `attrs` is the raw attribute text.
    Start { name: &'a str, attrs: &'a str, self_closing: bool },
    End { name: &'a str },
    /// Raw text; entities are still encoded (see [`decode_entities`]).
    Text(&'a str),
    /// CDATA content, already literal.
    CData(&'a str),
}

pub struct Tokenizer<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Tokenizer<'a> {
    pub fn new(src: &'a str) -> Self {
        // Skip a UTF-8 BOM.
        let src = src.strip_prefix('\u{feff}').unwrap_or(src);
        Tokenizer { src, pos: 0 }
    }
}

/// Strips any namespace prefix: `dc:title` -> `title`.
pub fn local_name(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        loop {
            let rest = &self.src[self.pos..];
            if rest.is_empty() {
                return None;
            }
            if !rest.starts_with('<') {
                let end = rest.find('<').unwrap_or(rest.len());
                self.pos += end;
                return Some(Token::Text(&rest[..end]));
            }
            if let Some(body) = rest.strip_prefix("<!--") {
                let end = body.find("-->").map(|i| i + 3).unwrap_or(body.len());
                self.pos += 4 + end;
                continue;
            }
            if let Some(body) = rest.strip_prefix("<![CDATA[") {
                let end = body.find("]]>").unwrap_or(body.len());
                self.pos += 9 + (end + 3).min(body.len());
                return Some(Token::CData(&body[..end]));
            }
            if rest.starts_with("<!") || rest.starts_with("<?") {
                let end = rest.find('>').map(|i| i + 1).unwrap_or(rest.len());
                self.pos += end;
                continue;
            }
            // A regular tag. Find the closing '>' while respecting quotes.
            let bytes = rest.as_bytes();
            let mut i = 1;
            let mut quote = 0u8;
            while i < bytes.len() {
                let c = bytes[i];
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'>' {
                    break;
                }
                i += 1;
            }
            let inner = &rest[1..i.min(rest.len())];
            self.pos += (i + 1).min(rest.len());
            if let Some(name) = inner.strip_prefix('/') {
                return Some(Token::End { name: name.trim() });
            }
            let (inner, self_closing) = match inner.strip_suffix('/') {
                Some(s) => (s, true),
                None => (inner, false),
            };
            let name_end = inner
                .find(|c: char| c.is_ascii_whitespace())
                .unwrap_or(inner.len());
            let name = &inner[..name_end];
            if name.is_empty() {
                // A stray '<' in text.
                return Some(Token::Text("<"));
            }
            return Some(Token::Start { name, attrs: &inner[name_end..], self_closing });
        }
    }
}

/// Finds an attribute value (raw, entities not decoded) by local name.
pub fn attr<'a>(attrs: &'a str, want: &str) -> Option<&'a str> {
    let b = attrs.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let ks = i;
        while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = &attrs[ks..i];
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            if key.is_empty() {
                i += 1;
            }
            continue;
        }
        i += 1;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let val = if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
            let q = b[i];
            let vs = i + 1;
            i = vs;
            while i < b.len() && b[i] != q {
                i += 1;
            }
            let v = &attrs[vs..i];
            i += 1;
            v
        } else {
            let vs = i;
            while i < b.len() && !b[i].is_ascii_whitespace() {
                i += 1;
            }
            &attrs[vs..i]
        };
        if local_name(key).eq_ignore_ascii_case(want) {
            return Some(val);
        }
    }
    None
}

fn named_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "shy" => '\u{ad}',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "lsquo" => '‘',
        "rsquo" => '’',
        "sbquo" => '‚',
        "ldquo" => '“',
        "rdquo" => '”',
        "bdquo" => '„',
        "laquo" => '«',
        "raquo" => '»',
        "bull" => '•',
        "middot" => '·',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "deg" => '°',
        "sect" => '§',
        "para" => '¶',
        "dagger" => '†',
        "Dagger" => '‡',
        "times" => '×',
        "divide" => '÷',
        "minus" => '−',
        "prime" => '′',
        "Prime" => '″',
        "thinsp" => '\u{2009}',
        "ensp" => '\u{2002}',
        "emsp" => '\u{2003}',
        "zwnj" => '\u{200c}',
        "zwj" => '\u{200d}',
        "eacute" => 'é',
        "egrave" => 'è',
        "ecirc" => 'ê',
        "euml" => 'ë',
        "aacute" => 'á',
        "agrave" => 'à',
        "acirc" => 'â',
        "auml" => 'ä',
        "aring" => 'å',
        "aelig" => 'æ',
        "ccedil" => 'ç',
        "iacute" => 'í',
        "igrave" => 'ì',
        "icirc" => 'î',
        "iuml" => 'ï',
        "ntilde" => 'ñ',
        "oacute" => 'ó',
        "ograve" => 'ò',
        "ocirc" => 'ô',
        "ouml" => 'ö',
        "oslash" => 'ø',
        "uacute" => 'ú',
        "ugrave" => 'ù',
        "ucirc" => 'û',
        "uuml" => 'ü',
        "szlig" => 'ß',
        "Eacute" => 'É',
        "Agrave" => 'À',
        "Auml" => 'Ä',
        "Ouml" => 'Ö',
        "Uuml" => 'Ü',
        "pound" => '£',
        "euro" => '€',
        "cent" => '¢',
        "frac12" => '½',
        "frac14" => '¼',
        "frac34" => '¾',
        "iexcl" => '¡',
        "iquest" => '¿',
        _ => return None,
    })
}

/// Appends `s` to `out` with XML/HTML entities decoded.
pub fn decode_entities_into(s: &str, out: &mut String) {
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let semi = after.find(';').filter(|&i| i <= 10);
        let decoded = semi.and_then(|i| {
            let name = &after[..i];
            let c = if let Some(num) = name.strip_prefix('#') {
                let n = match num.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => num.parse().ok(),
                };
                n.and_then(char::from_u32)
            } else {
                named_entity(name)
            };
            c.map(|c| (c, i))
        });
        match decoded {
            Some((c, i)) => {
                out.push(c);
                rest = &after[i + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
}

pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    decode_entities_into(s, &mut out);
    out
}
