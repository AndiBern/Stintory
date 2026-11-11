// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A small, tolerant parser for iRacing's session info YAML.
//!
//! iRacing writes a simple, indentation-based YAML subset (maps, `- ` lists,
//! `key: value` scalars). Values are not always valid YAML (e.g. unquoted
//! colons in team names), so instead of a strict YAML library we parse the
//! subset leniently: a value is everything after the first `": "`.

/// A parsed YAML node.
#[derive(Debug, Clone, PartialEq)]
pub enum Yaml {
    /// A scalar value (always kept as text; quotes removed)
    Scalar(String),
    /// A mapping, in document order
    Map(Vec<(String, Yaml)>),
    /// A list
    List(Vec<Yaml>),
    /// A key without value
    Null,
}

impl Yaml {
    /// Value of `key` in a map.
    pub fn get(&self, key: &str) -> Option<&Yaml> {
        match self {
            Yaml::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Follows a dot-separated path of map keys, e.g. `WeekendInfo.TrackID`.
    pub fn path(&self, path: &str) -> Option<&Yaml> {
        path.split('.').try_fold(self, |node, key| node.get(key))
    }

    /// The scalar text (None for empty strings and non-scalars).
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Yaml::Scalar(s) if !s.is_empty() => Some(s),
            _ => None,
        }
    }

    /// The leading number of a scalar, e.g. `4.7353 km` → 4.7353.
    pub fn as_f64(&self) -> Option<f64> {
        let s = self.as_str()?.trim();
        let end = s
            .char_indices()
            .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || ((c == '-' || c == '+') && i == 0)))
            .map_or(s.len(), |(i, _)| i);
        s[..end].parse().ok()
    }

    /// The scalar as integer (accepts `12`, `12.0`, `0x0a` is not supported).
    pub fn as_i64(&self) -> Option<i64> {
        let s = self.as_str()?.trim();
        s.parse::<i64>()
            .ok()
            .or_else(|| self.as_f64().map(|f| f as i64))
    }

    /// List items (empty for non-lists).
    pub fn items(&self) -> &[Yaml] {
        match self {
            Yaml::List(items) => items,
            _ => &[],
        }
    }

    /// Converts to JSON (scalars become strings).
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Yaml::Scalar(s) => serde_json::Value::String(s.clone()),
            Yaml::Map(entries) => serde_json::Value::Object(
                entries
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_json()))
                    .collect(),
            ),
            Yaml::List(items) => {
                serde_json::Value::Array(items.iter().map(Yaml::to_json).collect())
            }
            Yaml::Null => serde_json::Value::Null,
        }
    }
}

#[derive(Debug, Clone)]
struct Line {
    indent: usize,
    item: bool,
    content: String,
}

/// Parses iRacing session info YAML. Never fails; malformed lines are
/// skipped.
pub fn parse(text: &str) -> Yaml {
    let mut lines: Vec<Line> = text
        .lines()
        .filter_map(|raw| {
            let raw = raw.trim_end_matches(['\r', '\0']);
            let trimmed = raw.trim_start_matches(' ');
            if trimmed.is_empty()
                || trimmed == "---"
                || trimmed == "..."
                || trimmed.starts_with('#')
            {
                return None;
            }
            let indent = raw.len() - trimmed.len();
            let (item, content) = match trimmed.strip_prefix("- ") {
                Some(rest) => (true, rest.to_owned()),
                None if trimmed == "-" => (true, String::new()),
                None => (false, trimmed.to_owned()),
            };
            Some(Line {
                indent,
                item,
                content,
            })
        })
        .collect();
    let mut pos = 0;
    if lines.is_empty() {
        return Yaml::Map(Vec::new());
    }
    let indent = lines[0].indent;
    parse_node(&mut lines, &mut pos, indent)
}

fn parse_node(lines: &mut [Line], pos: &mut usize, indent: usize) -> Yaml {
    match lines.get(*pos) {
        Some(l) if l.item => parse_list(lines, pos, indent),
        Some(_) => parse_map(lines, pos, indent),
        None => Yaml::Null,
    }
}

fn parse_map(lines: &mut [Line], pos: &mut usize, indent: usize) -> Yaml {
    let mut entries = Vec::new();
    while let Some(line) = lines.get(*pos) {
        if line.indent < indent || (line.indent == indent && line.item) {
            break;
        }
        if line.indent > indent {
            // unexpected deeper line: skip (tolerance)
            *pos += 1;
            continue;
        }
        let (key, value) = split_key_value(&line.content);
        let (key, value) = (key.to_owned(), value.map(unquote));
        *pos += 1;
        let node = match value {
            Some(v) => Yaml::Scalar(v),
            None => match lines.get(*pos) {
                Some(next) if next.indent > indent || (next.indent == indent && next.item) => {
                    let child_indent = next.indent;
                    parse_node(lines, pos, child_indent)
                }
                _ => Yaml::Null,
            },
        };
        entries.push((key, node));
    }
    Yaml::Map(entries)
}

fn parse_list(lines: &mut [Line], pos: &mut usize, indent: usize) -> Yaml {
    let mut items = Vec::new();
    while let Some(line) = lines.get(*pos) {
        if line.indent != indent || !line.item {
            if line.indent > indent {
                *pos += 1;
                continue;
            }
            break;
        }
        let starts_map = split_key_value(&line.content).1.is_some() || line.content.ends_with(':');
        let scalar = line.content.clone();
        if line.content.is_empty() {
            *pos += 1;
            let child = match lines.get(*pos) {
                Some(next) if next.indent > indent => {
                    let child_indent = next.indent;
                    parse_node(lines, pos, child_indent)
                }
                _ => Yaml::Null,
            };
            items.push(child);
        } else if starts_map {
            // "- Key: value" starts a map whose entries are indented by 2
            let item_indent = indent + 2;
            lines[*pos].indent = item_indent;
            lines[*pos].item = false;
            items.push(parse_map(lines, pos, item_indent));
        } else {
            items.push(Yaml::Scalar(unquote(&scalar)));
            *pos += 1;
        }
    }
    Yaml::List(items)
}

/// Splits `key: value` at the first `": "`. `key:` yields `(key, None)`.
fn split_key_value(content: &str) -> (&str, Option<&str>) {
    if let Some(i) = content.find(": ") {
        let value = content[i + 2..].trim();
        (content[..i].trim(), Some(value))
    } else if let Some(key) = content.strip_suffix(':') {
        (key.trim(), None)
    } else {
        (content.trim(), Some(""))
    }
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    let quoted = v.len() >= 2
        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')));
    if quoted {
        v[1..v.len() - 1].to_owned()
    } else {
        v.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---
WeekendInfo:
 TrackName: snetterton 300
 TrackID: 297
 TrackLength: 4.7353 km
 TelemetryOptions:
  TelemetryDiskFile: \"\"

SessionInfo:
 CurrentSessionNum: 1
 Sessions:
 - SessionNum: 0
   SessionType: Practice
   ResultsPositions:
   - Position: 1
     CarIdx: 0
   - Position: 2
     CarIdx: 1
 - SessionNum: 1
   SessionType: Race
   ResultsPositions:

DriverInfo:
 DriverCarIdx: 12
 DriverTires:
 - TireIndex: 0
   TireCompoundType: \"Hard\"
 Drivers:
 - CarIdx: 0
   UserName: Some Body
   TeamName: Team: With Colon
   CarNumber: \"1\"
...
";

    #[test]
    fn parses_iracing_structure() {
        let y = parse(SAMPLE);
        assert_eq!(y.path("WeekendInfo.TrackID").unwrap().as_i64(), Some(297));
        assert_eq!(
            y.path("WeekendInfo.TrackLength").unwrap().as_f64(),
            Some(4.7353)
        );
        assert_eq!(
            y.path("WeekendInfo.TelemetryOptions.TelemetryDiskFile"),
            Some(&Yaml::Scalar(String::new()))
        );
        let sessions = y.path("SessionInfo.Sessions").unwrap().items();
        assert_eq!(sessions.len(), 2);
        assert_eq!(
            sessions[1].get("SessionType").unwrap().as_str(),
            Some("Race")
        );
        assert_eq!(
            sessions[0].get("ResultsPositions").unwrap().items().len(),
            2
        );
        assert_eq!(sessions[1].get("ResultsPositions"), Some(&Yaml::Null));
        let driver = &y.path("DriverInfo.Drivers").unwrap().items()[0];
        assert_eq!(
            driver.get("TeamName").unwrap().as_str(),
            Some("Team: With Colon")
        );
        assert_eq!(driver.get("CarNumber").unwrap().as_str(), Some("1"));
        assert_eq!(
            y.path("DriverInfo.DriverTires").unwrap().items()[0]
                .get("TireCompoundType")
                .unwrap()
                .as_str(),
            Some("Hard")
        );
        assert_eq!(
            y.path("DriverInfo.DriverCarIdx").unwrap().as_i64(),
            Some(12)
        );
    }

    #[test]
    fn numbers_with_units() {
        let v = |s: &str| Yaml::Scalar(s.into()).as_f64();
        assert_eq!(v("18.70 C"), Some(18.7));
        assert_eq!(v("-1.0000"), Some(-1.0));
        assert_eq!(v("unlimited"), None);
        assert_eq!(Yaml::Scalar("180.0000 sec".into()).as_i64(), Some(180));
    }

    #[test]
    fn tolerates_garbage() {
        let y = parse("A:\n   weird deeper line\n B: 1\nC: 2\n");
        assert_eq!(y.get("C").unwrap().as_i64(), Some(2));
        assert_eq!(parse(""), Yaml::Map(vec![]));
    }

    #[test]
    fn to_json_keeps_structure() {
        let y = parse(SAMPLE);
        let j = y.path("SessionInfo").unwrap().to_json();
        assert_eq!(j["Sessions"][1]["SessionType"], "Race");
    }
}
