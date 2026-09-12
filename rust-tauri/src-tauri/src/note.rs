use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

mod go_time {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn format(dt: &DateTime<Utc>) -> String {
        let base = dt.format("%Y-%m-%dT%H:%M:%S").to_string();
        let nanos = dt.timestamp_subsec_nanos();
        if nanos == 0 {
            format!("{base}Z")
        } else {
            let frac = format!("{nanos:09}");
            let frac = frac.trim_end_matches('0');
            format!("{base}.{frac}Z")
        }
    }

    fn parse<E: serde::de::Error>(s: &str) -> Result<DateTime<Utc>, E> {
        DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(serde::de::Error::custom)
    }

    pub fn serialize<S: Serializer>(dt: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format(dt))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime<Utc>, D::Error> {
        let s = String::deserialize(d)?;
        parse(&s)
    }

    pub mod opt {
        use super::{format, parse, DateTime, Utc};
        use serde::{Deserialize, Deserializer, Serializer};

        pub fn serialize<S: Serializer>(
            v: &Option<DateTime<Utc>>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            match v {
                Some(dt) => s.serialize_str(&format(dt)),
                None => s.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<DateTime<Utc>>, D::Error> {
            match Option::<String>::deserialize(d)? {
                None => Ok(None),
                Some(s) => parse(&s).map(Some),
            }
        }
    }
}

fn escape_html(bytes: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => {
                out.extend_from_slice(b"\\u003c");
                i += 1;
            }
            b'>' => {
                out.extend_from_slice(b"\\u003e");
                i += 1;
            }
            b'&' => {
                out.extend_from_slice(b"\\u0026");
                i += 1;
            }
            0xE2 if i + 2 < bytes.len() && bytes[i + 1] == 0x80 && bytes[i + 2] == 0xA8 => {
                out.extend_from_slice(b"\\u2028");
                i += 3;
            }
            0xE2 if i + 2 < bytes.len() && bytes[i + 1] == 0x80 && bytes[i + 2] == 0xA9 => {
                out.extend_from_slice(b"\\u2029");
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_empty_string(s: &String) -> bool {
    s.is_empty()
}

fn null_to_empty_vec<'de, D>(d: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<Vec<String>>::deserialize(d)?;
    Ok(opt.unwrap_or_default())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub title: String,
    #[serde(default, deserialize_with = "null_to_empty_vec")]
    pub tags: Vec<String>,
    #[serde(serialize_with = "go_time::serialize", deserialize_with = "go_time::deserialize")]
    pub created: DateTime<Utc>,
    #[serde(serialize_with = "go_time::serialize", deserialize_with = "go_time::deserialize")]
    pub updated: DateTime<Utc>,
    pub body: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub trashed: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "go_time::opt::serialize",
        deserialize_with = "go_time::opt::deserialize"
    )]
    pub trashed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "is_empty_string")]
    pub filetype: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteMeta {
    pub id: String,
    pub title: String,
    #[serde(default, deserialize_with = "null_to_empty_vec")]
    pub tags: Vec<String>,
    #[serde(serialize_with = "go_time::serialize", deserialize_with = "go_time::deserialize")]
    pub created: DateTime<Utc>,
    #[serde(serialize_with = "go_time::serialize", deserialize_with = "go_time::deserialize")]
    pub updated: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub trashed: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "go_time::opt::serialize",
        deserialize_with = "go_time::opt::deserialize"
    )]
    pub trashed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub has_conflict: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub sync_failed: bool,
    #[serde(default, skip_serializing_if = "is_empty_string")]
    pub filetype: String,
}

pub fn decode(b: &[u8]) -> Result<Note, serde_json::Error> {
    serde_json::from_slice(b)
}

fn normalize_line_endings(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

impl Note {
    pub fn new(
        id: String,
        title: String,
        tags: Vec<String>,
        created: DateTime<Utc>,
        updated: DateTime<Utc>,
        body: String,
    ) -> Note {
        Note {
            id,
            title,
            tags,
            created,
            updated,
            body,
            pinned: false,
            trashed: false,
            trashed_at: None,
            filetype: String::new(),
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut c = self.clone();
        c.body = normalize_line_endings(&c.body);
        let bytes = serde_json::to_vec_pretty(&c)?;
        Ok(escape_html(bytes))
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    pub fn meta(&self) -> NoteMeta {
        NoteMeta {
            id: self.id.clone(),
            title: meta_title(self),
            tags: self.tags.clone(),
            created: self.created,
            updated: self.updated,
            pinned: self.pinned,
            trashed: self.trashed,
            trashed_at: self.trashed_at,
            has_conflict: has_conflict_markers(&self.body),
            sync_failed: false,
            filetype: self.filetype.clone(),
        }
    }
}

impl NoteMeta {
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }
}

pub fn first_line_title(body: &str) -> String {
    for raw in body.split('\n') {
        if raw.trim().is_empty() {
            continue;
        }
        let mut line = raw.trim_start_matches([' ', '\t']);
        while let Some(stripped) = line.strip_prefix('#') {
            line = stripped;
        }
        return line.trim().to_string();
    }
    String::new()
}

fn meta_title(n: &Note) -> String {
    match n.filetype.as_str() {
        "json" | "sh" => n.title.clone(),
        _ => {
            let t = first_line_title(&n.body);
            if !t.is_empty() {
                t
            } else {
                n.title.clone()
            }
        }
    }
}

fn has_conflict_markers(body: &str) -> bool {
    let mut has_local = false;
    let mut has_remote = false;
    for line in body.split('\n') {
        if line == "<<<<<<< local version" {
            has_local = true;
        } else if line == ">>>>>>> remote version" {
            has_remote = true;
        }
        if has_local && has_remote {
            return true;
        }
    }
    false
}

pub fn filter<'a>(notes: &'a [Note], tag: &str) -> Vec<&'a Note> {
    notes.iter().filter(|n| n.has_tag(tag)).collect()
}

pub fn filter_meta<'a>(metas: &'a [NoteMeta], tag: &str) -> Vec<&'a NoteMeta> {
    metas.iter().filter(|m| m.has_tag(tag)).collect()
}

pub fn all_tags(notes: &[Note]) -> Vec<String> {
    let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for n in notes {
        for t in &n.tags {
            set.insert(t.clone());
        }
    }
    set.into_iter().collect()
}

pub fn all_meta_tags(metas: &[NoteMeta]) -> Vec<String> {
    let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for m in metas {
        for t in &m.tags {
            set.insert(t.clone());
        }
    }
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};

    fn sample(id: &str, title: &str, tags: &[&str]) -> Note {
        Note {
            id: id.to_string(),
            title: title.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            created: Utc.with_ymd_and_hms(2026, 9, 1, 14, 30, 22).unwrap(),
            updated: Utc.with_ymd_and_hms(2026, 9, 1, 15, 4, 11).unwrap(),
            body: "# 正文\n内容".to_string(),
            pinned: false,
            trashed: false,
            trashed_at: None,
            filetype: String::new(),
        }
    }

    #[test]
    fn encode_decode_round_trip() {
        let input = sample("n1", "大柴胡汤心得", &["医案", "少阳病"]);
        let b = input.encode().expect("encode");
        let out = decode(&b).expect("decode");
        assert_eq!(out.id, input.id);
        assert_eq!(out.title, input.title);
        assert_eq!(out.body, input.body);
        assert_eq!(out.tags, vec!["医案".to_string(), "少阳病".to_string()]);
        assert_eq!(out.created, input.created);
        assert_eq!(out.updated, input.updated);
    }

    #[test]
    fn decode_invalid() {
        assert!(decode(b"not json").is_err());
    }

    #[test]
    fn has_tag_and_filter() {
        let a = sample("a", "A", &["医案", "太阳病"]);
        let b = sample("b", "B", &["笔记"]);
        let c = sample("c", "C", &["医案"]);
        assert!(a.has_tag("太阳病"));
        assert!(!a.has_tag("少阳病"));
        let notes = vec![a, b, c];
        let got = filter(&notes, "医案");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].id, "a");
        assert_eq!(got[1].id, "c");
        assert_eq!(filter(&notes, "无此tag").len(), 0);
    }

    #[test]
    fn all_tags_sorted_unique() {
        let a = sample("a", "A", &["医案", "太阳病"]);
        let b = sample("b", "B", &["医案", "笔记"]);
        let got = all_tags(&[a, b]);
        let mut want = vec!["医案".to_string(), "太阳病".to_string(), "笔记".to_string()];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn meta_has_conflict() {
        let cases: &[(&str, &str, bool)] = &[
            ("clean", "just body\nno markers\n", false),
            ("only local marker", "<<<<<<< local version\ntext\n", false),
            ("only remote marker", "text\n>>>>>>> remote version\n", false),
            (
                "both markers",
                "line\n<<<<<<< local version\nlocal\n=======\nremote\n>>>>>>> remote version\ntail\n",
                true,
            ),
        ];
        for (name, body, want) in cases {
            let n = Note {
                id: "n".to_string(),
                title: String::new(),
                tags: vec![],
                created: Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap(),
                updated: Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap(),
                body: body.to_string(),
                pinned: false,
                trashed: false,
                trashed_at: None,
                filetype: String::new(),
            };
            assert_eq!(n.meta().has_conflict, *want, "case {name}");
        }
    }

    #[test]
    fn encode_normalizes_line_endings() {
        let n = Note {
            id: "n1".to_string(),
            title: "t".to_string(),
            tags: vec![],
            created: Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap(),
            updated: Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap(),
            body: "a\r\nb\rc\n".to_string(),
            pinned: false,
            trashed: false,
            trashed_at: None,
            filetype: String::new(),
        };
        let b = n.encode().expect("encode");
        assert!(!b.contains(&b'\r'), "encoded bytes still contain CR");
        assert_eq!(n.body, "a\r\nb\rc\n", "in-memory body must not be mutated");
    }

    fn note_at(created: DateTime<Utc>, body: &str) -> Note {
        Note {
            id: "20260909-145142-e5a3".to_string(),
            title: "t".to_string(),
            tags: vec![],
            created,
            updated: created,
            body: body.to_string(),
            pinned: false,
            trashed: false,
            trashed_at: None,
            filetype: String::new(),
        }
    }

    fn created_field(n: &Note) -> String {
        let b = n.encode().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
        v["created"].as_str().unwrap().to_string()
    }

    #[test]
    fn encode_escapes_html_like_go() {
        let n = note_at(Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap(), "a > b < c & d");
        let s = String::from_utf8(n.encode().unwrap()).unwrap();
        assert!(s.contains("\\u003e"), "> must escape: {s}");
        assert!(s.contains("\\u003c"), "< must escape");
        assert!(s.contains("\\u0026"), "& must escape");
        assert!(!s.contains("a > b"), "no literal < > & in output");
    }

    #[test]
    fn encode_timestamp_matches_go_rfc3339nano() {
        let with_ns = |ns: u32| {
            Utc.with_ymd_and_hms(2026, 9, 9, 14, 51, 42)
                .unwrap()
                .with_nanosecond(ns)
                .unwrap()
        };
        assert_eq!(
            created_field(&note_at(with_ns(599_176_000), "x")),
            "2026-09-09T14:51:42.599176Z"
        );
        assert_eq!(
            created_field(&note_at(with_ns(120_000_000), "x")),
            "2026-09-09T14:51:42.12Z"
        );
        assert_eq!(
            created_field(&note_at(with_ns(100_000_000), "x")),
            "2026-09-09T14:51:42.1Z"
        );
        assert_eq!(
            created_field(&note_at(with_ns(123_456_789), "x")),
            "2026-09-09T14:51:42.123456789Z"
        );
        assert_eq!(
            created_field(&note_at(with_ns(0), "x")),
            "2026-09-09T14:51:42Z"
        );
    }

    #[test]
    fn timestamp_round_trips() {
        let n = note_at(
            Utc.with_ymd_and_hms(2026, 9, 9, 14, 51, 42)
                .unwrap()
                .with_nanosecond(599_176_000)
                .unwrap(),
            "x",
        );
        let b = n.encode().unwrap();
        let out = decode(&b).unwrap();
        assert_eq!(out.created, n.created);
    }
}

