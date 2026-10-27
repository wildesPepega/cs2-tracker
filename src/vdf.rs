//! Minimal parser for Valve's text KeyValues format (as returned by PICS appinfo).

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Str(String),
    Obj(BTreeMap<String, Node>),
}

#[cfg(test)]
impl Node {
    pub fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Obj(map) => map.get(key),
            Node::Str(_) => None,
        }
    }

    /// Follows a `/`-separated path, e.g. `"depots/branches/public/buildid"`.
    pub fn path(&self, path: &str) -> Option<&Node> {
        path.split('/').try_fold(self, |node, key| node.get(key))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Node::Str(s) => Some(s),
            Node::Obj(_) => None,
        }
    }
}

/// Parses a KeyValues document and returns the value of its root key.
pub fn parse(text: &str) -> Option<Node> {
    let mut chars = text.chars().peekable();
    let mut root = parse_obj(&mut chars, true)?;
    // Documents have a single root key ("appinfo"); unwrap it.
    match &mut root {
        Node::Obj(map) if map.len() == 1 => map.pop_first().map(|(_, v)| v),
        _ => Some(root),
    }
}

type Chars<'a> = std::iter::Peekable<std::str::Chars<'a>>;

fn parse_obj(chars: &mut Chars, top_level: bool) -> Option<Node> {
    let mut map = BTreeMap::new();
    loop {
        skip_ws(chars);
        match chars.peek()? {
            '}' if !top_level => {
                chars.next();
                return Some(Node::Obj(map));
            }
            '\0' => return Some(Node::Obj(map)),
            _ => {}
        }
        let key = parse_string(chars)?;
        skip_ws(chars);
        let value = if chars.peek() == Some(&'{') {
            chars.next();
            parse_obj(chars, false)?
        } else {
            Node::Str(parse_string(chars)?)
        };
        map.insert(key, value);
        if top_level {
            skip_ws(chars);
            if matches!(chars.peek(), None | Some('\0')) {
                return Some(Node::Obj(map));
            }
        }
    }
}

fn skip_ws(chars: &mut Chars) {
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '/' {
            // `//` line comment
            chars.next();
            for c in chars.by_ref() {
                if c == '\n' {
                    break;
                }
            }
        } else {
            break;
        }
    }
}

fn parse_string(chars: &mut Chars) -> Option<String> {
    let mut out = String::new();
    if chars.peek() == Some(&'"') {
        chars.next();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(out),
                '\\' => match chars.next()? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    other => out.push(other),
                },
                c => out.push(c),
            }
        }
        None
    } else {
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() || c == '{' || c == '}' || c == '"' {
                break;
            }
            out.push(c);
            chars.next();
        }
        (!out.is_empty()).then_some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_appinfo() {
        let text = "\"appinfo\"\n{\n\t\"appid\"\t\t\"730\"\n\t\"depots\"\n\t{\n\t\t\"branches\"\n\t\t{\n\t\t\t\"public\"\n\t\t\t{\n\t\t\t\t\"buildid\"\t\t\"123\"\n\t\t\t}\n\t\t}\n\t}\n}\n\0";
        let root = parse(text).unwrap();
        assert_eq!(root.path("appid").and_then(Node::as_str), Some("730"));
        assert_eq!(
            root.path("depots/branches/public/buildid").and_then(Node::as_str),
            Some("123")
        );
    }

    #[test]
    fn handles_escapes() {
        let root = parse(r#""a" { "k" "say \"hi\"" }"#).unwrap();
        assert_eq!(root.path("k").and_then(Node::as_str), Some("say \"hi\""));
    }
}
