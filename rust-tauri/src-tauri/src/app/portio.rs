use super::App;
use crate::local::StoreError;
use crate::note;
use crate::note::NoteMeta;
use crate::sync::EngineError;
use regex::Regex;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportViewKind {
    All,
    Trash,
    Untagged,
    Tag,
}

pub struct ExportView {
    pub kind: ExportViewKind,
    pub tag: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Md,
    Txt,
}

fn io_err(e: std::io::Error) -> EngineError {
    EngineError::Store(StoreError::Io(e))
}

fn ext_lower(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default()
}

fn is_importable_ext(path: &Path) -> bool {
    matches!(ext_lower(path).as_str(), "md" | "txt" | "sh" | "json")
}

fn detect_file_type(path: &Path) -> String {
    match ext_lower(path).as_str() {
        "sh" => "sh".to_string(),
        "json" => "json".to_string(),
        _ => String::new(),
    }
}

fn shell_body_with_title_comment(body: &str, filename: &str) -> String {
    for raw in body.split('\n') {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            return body.to_string();
        }
        break;
    }
    let prefix = format!("# {filename}\n");
    if body.is_empty() {
        prefix
    } else {
        format!("{prefix}{body}")
    }
}

fn pretty_json(body: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_else(|_| body.to_string()),
        Err(_) => body.to_string(),
    }
}

fn is_importable_text(p: &Path) -> bool {
    let meta = match fs::metadata(p) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if meta.is_dir() {
        return false;
    }
    let b = match fs::read(p) {
        Ok(b) => b,
        Err(_) => return false,
    };
    if b.contains(&0) {
        return false;
    }
    std::str::from_utf8(&b).is_ok()
}

impl App {
    pub fn import_dir(&self, dir: &str) -> Result<Vec<String>, EngineError> {
        let entries = fs::read_dir(dir).map_err(io_err)?;
        let base = Path::new(dir)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let tags = if !base.is_empty() && base != "." && base != "/" {
            vec![base]
        } else {
            Vec::new()
        };
        let mut paths = Vec::new();
        for e in entries {
            let e = e.map_err(io_err)?;
            if e.file_type().map_err(io_err)?.is_dir() {
                continue;
            }
            let p = e.path();
            if is_importable_ext(&p) {
                paths.push(p);
            }
        }
        self.import_files_inner(paths, tags)
    }

    pub fn import_files(&self, paths: Vec<String>) -> Result<Vec<String>, EngineError> {
        let eligible: Vec<std::path::PathBuf> = paths
            .into_iter()
            .map(std::path::PathBuf::from)
            .filter(|p| is_importable_text(p))
            .collect();
        self.import_files_inner(eligible, Vec::new())
    }

    fn import_files_inner(
        &self,
        paths: Vec<std::path::PathBuf>,
        tags: Vec<String>,
    ) -> Result<Vec<String>, EngineError> {
        let mut ids = Vec::with_capacity(paths.len());
        for p in paths {
            let b = fs::read(&p).map_err(io_err)?;
            let ft = detect_file_type(&p);
            let filename = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let mut body = String::from_utf8_lossy(&b).to_string();
            let title;
            match ft.as_str() {
                "sh" => {
                    body = shell_body_with_title_comment(&body, &filename);
                    title = filename.clone();
                }
                "json" => {
                    body = pretty_json(&body);
                    title = filename.clone();
                }
                _ => {
                    let t = note::first_line_title(&body);
                    title = if t.is_empty() { filename.clone() } else { t };
                }
            }
            let n = self.store().create(&title, tags.clone(), &body)?;
            if !ft.is_empty() {
                self.store().set_file_type(&n.id, &ft)?;
            }
            self.syncer.mark_unsynced(&n.id);
            ids.push(n.id);
        }
        Ok(ids)
    }

    pub fn export_view(
        &self,
        dest_dir: &str,
        view: &ExportView,
        format: ExportFormat,
    ) -> Result<usize, EngineError> {
        if dest_dir.is_empty() {
            return Err(EngineError::Store(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "empty destination",
            ))));
        }
        let metas = self.store().list_meta()?;
        let (subdir, filter) = view_spec(view);
        let ext = ext_for_format(format);
        let mut sub = safe_name(&subdir);
        if sub.is_empty() {
            sub = "Export".to_string();
        }
        let out_dir = Path::new(dest_dir).join(&sub);
        fs::create_dir_all(&out_dir).map_err(io_err)?;
        let mut used = std::collections::HashSet::new();
        let mut count = 0;
        for m in &metas {
            if !filter(m) {
                continue;
            }
            let n = self.store().load(&m.id)?;
            let mut base = safe_name(&m.title);
            if base.is_empty() {
                base = "note".to_string();
            }
            let mut file_ext = ext;
            let mut body = n.body.clone();
            match n.filetype.as_str() {
                "sh" => file_ext = ".sh",
                "json" => file_ext = ".json",
                _ => {
                    if format == ExportFormat::Txt {
                        body = strip_markdown(&body);
                    }
                }
            }
            let name = unique_name(&used, &base, file_ext);
            used.insert(name.clone());
            fs::write(out_dir.join(&name), body.as_bytes()).map_err(io_err)?;
            count += 1;
        }
        Ok(count)
    }
}

type Filter = Box<dyn Fn(&NoteMeta) -> bool>;

fn view_spec(view: &ExportView) -> (String, Filter) {
    match view.kind {
        ExportViewKind::All => ("All".to_string(), Box::new(|m: &NoteMeta| !m.trashed)),
        ExportViewKind::Trash => ("Trash".to_string(), Box::new(|m: &NoteMeta| m.trashed)),
        ExportViewKind::Untagged => (
            "Untagged".to_string(),
            Box::new(|m: &NoteMeta| !m.trashed && m.tags.is_empty()),
        ),
        ExportViewKind::Tag => {
            let tag = view.tag.clone();
            (
                view.tag.clone(),
                Box::new(move |m: &NoteMeta| !m.trashed && m.tags.iter().any(|t| *t == tag)),
            )
        }
    }
}

fn ext_for_format(f: ExportFormat) -> &'static str {
    match f {
        ExportFormat::Md => ".md",
        ExportFormat::Txt => ".txt",
    }
}

fn safe_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for r in s.chars() {
        match r {
            c if (c as u32) < 0x20 => out.push('_'),
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => out.push('_'),
            c => out.push(c),
        }
    }
    let trimmed: String = out
        .trim_matches(|c: char| c.is_whitespace() || c == '.')
        .to_string();
    let runes: Vec<char> = trimmed.chars().collect();
    if runes.len() > 100 {
        runes[..100].iter().collect()
    } else {
        trimmed
    }
}

fn unique_name(used: &std::collections::HashSet<String>, base: &str, ext: &str) -> String {
    let name = format!("{base}{ext}");
    if !used.contains(&name) {
        return name;
    }
    let mut i = 2;
    loop {
        let candidate = format!("{base}-{i}{ext}");
        if !used.contains(&candidate) {
            return candidate;
        }
        i += 1;
    }
}

struct MdRegexes {
    image: Regex,
    inline_img: Regex,
    link: Regex,
    ref_link: Regex,
    autolink: Regex,
    inline_code: Regex,
    bold_star: Regex,
    bold_us: Regex,
    italic_s: Regex,
    italic_u: Regex,
    strike: Regex,
    html_tag: Regex,
    table_sep: Regex,
}

fn md() -> &'static MdRegexes {
    static R: OnceLock<MdRegexes> = OnceLock::new();
    R.get_or_init(|| MdRegexes {
        image: Regex::new(r"!\[([^\]]*)\]\([^)]*\)").unwrap(),
        inline_img: Regex::new(r"!\[([^\]]*)\]\[[^\]]*\]").unwrap(),
        link: Regex::new(r"\[([^\]]*)\]\(([^)]*)\)").unwrap(),
        ref_link: Regex::new(r"\[([^\]]*)\]\[[^\]]*\]").unwrap(),
        autolink: Regex::new(r"<((?:https?|ftp|mailto):[^>]+)>").unwrap(),
        inline_code: Regex::new(r"`+([^`]+?)`+").unwrap(),
        bold_star: Regex::new(r"\*\*\*?(.+?)\*\*\*?").unwrap(),
        bold_us: Regex::new(r"__(.+?)__").unwrap(),
        italic_s: Regex::new(r"\*([^*\n]+?)\*").unwrap(),
        italic_u: Regex::new(r"\b_([^_\n]+?)_\b").unwrap(),
        strike: Regex::new(r"~~([^~\n]+?)~~").unwrap(),
        html_tag: Regex::new(r"</?[a-zA-Z][^>]*>").unwrap(),
        table_sep: Regex::new(r"^\s*\|?[\s:\-]*\|[\s:\-|]*\|?\s*$").unwrap(),
    })
}

fn strip_markdown(s: &str) -> String {
    let lines: Vec<&str> = s.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut in_fence = false;
    for line in &lines {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            out.push(line.to_string());
            continue;
        }
        if is_horizontal_rule(trimmed) {
            continue;
        }
        if is_setext_underline(trimmed) && !out.is_empty() && !out.last().unwrap().trim().is_empty()
        {
            continue;
        }
        if md().table_sep.is_match(line) {
            continue;
        }
        let mut l = strip_atx_heading(line);
        l = strip_blockquote_prefix(&l);
        l = strip_list_marker(&l);
        l = strip_inline(&l);
        out.push(l);
    }
    out.join("\n")
}

fn is_horizontal_rule(trimmed: &str) -> bool {
    let bytes = trimmed.as_bytes();
    if bytes.len() < 3 {
        return false;
    }
    let c = bytes[0];
    if c != b'-' && c != b'*' && c != b'_' {
        return false;
    }
    for &b in bytes {
        if b == b' ' || b == b'\t' {
            continue;
        }
        if b != c {
            return false;
        }
    }
    true
}

fn is_setext_underline(trimmed: &str) -> bool {
    let bytes = trimmed.as_bytes();
    if bytes.len() < 2 {
        return false;
    }
    let c = bytes[0];
    if c != b'=' && c != b'-' {
        return false;
    }
    bytes.iter().all(|&b| b == c)
}

fn strip_atx_heading(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    let mut hashes = 0;
    while i + hashes < bytes.len() && bytes[i + hashes] == b'#' && hashes < 6 {
        hashes += 1;
    }
    if hashes == 0 {
        return line.to_string();
    }
    let rest = &line[i + hashes..];
    if rest.is_empty() {
        return String::new();
    }
    let rest_bytes = rest.as_bytes();
    if rest_bytes[0] != b' ' && rest_bytes[0] != b'\t' {
        return line.to_string();
    }
    let rest = rest.trim_start_matches([' ', '\t']);
    let rest = rest.trim_end_matches([' ', '\t']);
    let rest = rest.trim_end_matches('#');
    rest.trim_end_matches([' ', '\t']).to_string()
}

fn strip_blockquote_prefix(line: &str) -> String {
    let mut line = line.to_string();
    loop {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'>' {
            let mut j = i + 1;
            if j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            line = line[j..].to_string();
            continue;
        }
        return line;
    }
}

fn strip_list_marker(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if i >= bytes.len() {
        return line.to_string();
    }
    let c = bytes[i];
    if (c == b'-' || c == b'*' || c == b'+')
        && i + 1 < bytes.len()
        && (bytes[i + 1] == b' ' || bytes[i + 1] == b'\t')
    {
        let tail = line[i + 2..].trim_start_matches([' ', '\t']);
        return format!("{}{}", &line[..i], tail);
    }
    let mut j = i;
    while j < bytes.len() && bytes[j].is_ascii_digit() {
        j += 1;
    }
    if j > i
        && j < bytes.len()
        && (bytes[j] == b'.' || bytes[j] == b')')
        && j + 1 < bytes.len()
        && (bytes[j + 1] == b' ' || bytes[j + 1] == b'\t')
    {
        let tail = line[j + 2..].trim_start_matches([' ', '\t']);
        return format!("{}{}", &line[..i], tail);
    }
    line.to_string()
}

fn strip_inline(line: &str) -> String {
    let r = md();
    let mut s = hide_escapes(line);
    s = r.image.replace_all(&s, "$1").into_owned();
    s = r.inline_img.replace_all(&s, "$1").into_owned();
    s = r.link.replace_all(&s, "$1").into_owned();
    s = r.ref_link.replace_all(&s, "$1").into_owned();
    s = r.autolink.replace_all(&s, "$1").into_owned();
    s = r.inline_code.replace_all(&s, "$1").into_owned();
    s = r.bold_star.replace_all(&s, "$1").into_owned();
    s = r.bold_us.replace_all(&s, "$1").into_owned();
    s = r.italic_s.replace_all(&s, "$1").into_owned();
    s = r.italic_u.replace_all(&s, "$1").into_owned();
    s = r.strike.replace_all(&s, "$1").into_owned();
    s = r.html_tag.replace_all(&s, "").into_owned();
    unhide_escapes(&s)
}

fn is_escapable(c: u8) -> bool {
    matches!(
        c,
        b'\\'
            | b'`'
            | b'*'
            | b'_'
            | b'{'
            | b'}'
            | b'['
            | b']'
            | b'('
            | b')'
            | b'#'
            | b'+'
            | b'-'
            | b'.'
            | b'!'
            | b'~'
            | b'<'
            | b'>'
            | b'|'
    )
}

fn hide_escapes(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() && is_escapable(bytes[i + 1]) {
            let c = char::from_u32(0xE800 + bytes[i + 1] as u32).unwrap();
            out.push(c);
            i += 2;
            continue;
        }
        let ch_len = utf8_len(bytes[i]);
        out.push_str(&s[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

fn unhide_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for r in s.chars() {
        let u = r as u32;
        if (0xE800..=0xE87F).contains(&u) {
            out.push(char::from_u32(u - 0xE800).unwrap());
        } else {
            out.push(r);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::tests_support::test_app;
    use super::*;
    use std::fs;

    fn list_dir(dir: &Path) -> Vec<String> {
        let mut names = Vec::new();
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            if !e.file_type().unwrap().is_dir() {
                names.push(e.file_name().to_string_lossy().to_string());
            }
        }
        names
    }

    #[test]
    fn import_dir_reads_md_and_txt_only() {
        let a = test_app();
        let src = tempfile::tempdir().unwrap();
        let w = |name: &str, body: &str| {
            fs::write(src.path().join(name), body).unwrap();
        };
        w("a.md", "# Alpha\nbody a");
        w("b.txt", "Beta first line\nrest");
        w("c.MD", "Case md upper");
        w("d.TXT", "Case txt upper");
        w("e.png", "ignored image");
        w("readme", "no extension ignored");
        fs::create_dir(src.path().join("sub")).unwrap();
        fs::write(src.path().join("sub/nested.md"), "nope").unwrap();
        let ids = a.import_dir(src.path().to_str().unwrap()).unwrap();
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn import_dir_uses_dir_basename_as_tag() {
        let a = test_app();
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("医案");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("one.md"), "body").unwrap();
        let ids = a.import_dir(dir.to_str().unwrap()).unwrap();
        assert_eq!(ids.len(), 1);
        let loaded = a.load_note(&ids[0]).unwrap();
        assert_eq!(loaded.tags, vec!["医案".to_string()]);
        assert_eq!(loaded.body, "body");
    }

    #[test]
    fn import_files_has_no_tags() {
        let a = test_app();
        let src = tempfile::tempdir().unwrap();
        let p1 = src.path().join("a.md");
        let p2 = src.path().join("b.txt");
        fs::write(&p1, "hello").unwrap();
        fs::write(&p2, "world").unwrap();
        let ids = a
            .import_files(vec![
                p1.to_string_lossy().to_string(),
                p2.to_string_lossy().to_string(),
                src.path().join("no.png").to_string_lossy().to_string(),
            ])
            .unwrap();
        assert_eq!(ids.len(), 2);
        for id in ids {
            let n = a.load_note(&id).unwrap();
            assert!(n.tags.is_empty());
        }
    }

    #[test]
    fn export_view_all_only_non_trashed() {
        let a = test_app();
        a.create_note("", vec!["x".into()], "kept 1").unwrap();
        a.create_note("", vec![], "kept 2").unwrap();
        let trashed = a.create_note("", vec![], "gone").unwrap();
        a.set_trashed(&trashed.id, true).unwrap();
        let dest = tempfile::tempdir().unwrap();
        let n = a
            .export_view(
                dest.path().to_str().unwrap(),
                &ExportView {
                    kind: ExportViewKind::All,
                    tag: String::new(),
                },
                ExportFormat::Md,
            )
            .unwrap();
        assert_eq!(n, 2);
        let names = list_dir(&dest.path().join("All"));
        assert_eq!(names.len(), 2);
        assert!(names.iter().all(|n| n.ends_with(".md")));
    }

    #[test]
    fn export_view_trash_only_trashed() {
        let a = test_app();
        a.create_note("", vec![], "still here").unwrap();
        let trashed = a.create_note("", vec![], "gone body").unwrap();
        a.set_trashed(&trashed.id, true).unwrap();
        let dest = tempfile::tempdir().unwrap();
        let n = a
            .export_view(
                dest.path().to_str().unwrap(),
                &ExportView {
                    kind: ExportViewKind::Trash,
                    tag: String::new(),
                },
                ExportFormat::Txt,
            )
            .unwrap();
        assert_eq!(n, 1);
        let names = list_dir(&dest.path().join("Trash"));
        assert_eq!(names.len(), 1);
        assert!(names[0].ends_with(".txt"));
        let body = fs::read_to_string(dest.path().join("Trash").join(&names[0])).unwrap();
        assert_eq!(body, "gone body");
    }

    #[test]
    fn export_view_untagged_excludes_tags_and_trash() {
        let a = test_app();
        a.create_note("", vec!["x".into()], "has tag").unwrap();
        a.create_note("", vec![], "no tag kept").unwrap();
        let trashed = a.create_note("", vec![], "no tag trashed").unwrap();
        a.set_trashed(&trashed.id, true).unwrap();
        let dest = tempfile::tempdir().unwrap();
        let n = a
            .export_view(
                dest.path().to_str().unwrap(),
                &ExportView {
                    kind: ExportViewKind::Untagged,
                    tag: String::new(),
                },
                ExportFormat::Md,
            )
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(list_dir(&dest.path().join("Untagged")).len(), 1);
    }

    #[test]
    fn export_view_tag_picks_matching_non_trashed() {
        let a = test_app();
        a.create_note("", vec!["医案".into(), "笔记".into()], "kept 1")
            .unwrap();
        a.create_note("", vec!["医案".into()], "kept 2").unwrap();
        a.create_note("", vec!["其他".into()], "no match").unwrap();
        let trashed = a
            .create_note("", vec!["医案".into()], "trashed match")
            .unwrap();
        a.set_trashed(&trashed.id, true).unwrap();
        let dest = tempfile::tempdir().unwrap();
        let n = a
            .export_view(
                dest.path().to_str().unwrap(),
                &ExportView {
                    kind: ExportViewKind::Tag,
                    tag: "医案".into(),
                },
                ExportFormat::Md,
            )
            .unwrap();
        assert_eq!(n, 2);
        assert_eq!(list_dir(&dest.path().join("医案")).len(), 2);
    }

    #[test]
    fn export_duplicate_titles_get_suffixes() {
        let a = test_app();
        a.create_note("", vec![], "Same Title\nbody one").unwrap();
        a.create_note("", vec![], "Same Title\nbody two").unwrap();
        let dest = tempfile::tempdir().unwrap();
        let n = a
            .export_view(
                dest.path().to_str().unwrap(),
                &ExportView {
                    kind: ExportViewKind::All,
                    tag: String::new(),
                },
                ExportFormat::Md,
            )
            .unwrap();
        assert_eq!(n, 2);
        let names = list_dir(&dest.path().join("All"));
        assert!(names.iter().any(|s| s == "Same Title-2.md"));
    }

    #[test]
    fn safe_name_strips_metachars() {
        let cases = [
            ("plain", "plain"),
            ("has/slash", "has_slash"),
            ("back\\slash", "back_slash"),
            ("colon:in:name", "colon_in_name"),
            ("quote\"and*star", "quote_and_star"),
            ("pipe|less<greater>", "pipe_less_greater_"),
            ("tab\there", "tab_here"),
            ("null\x00byte", "null_byte"),
            ("  trim spaces  ", "trim spaces"),
            ("trailing.dots...", "trailing.dots"),
            ("question?mark", "question_mark"),
        ];
        for (i, w) in cases {
            assert_eq!(safe_name(i), w, "safe_name({i:?})");
        }
    }

    #[test]
    fn safe_name_caps_length() {
        let input = "a".repeat(200);
        assert_eq!(safe_name(&input).chars().count(), 100);
    }

    #[test]
    fn strip_markdown_headings() {
        let cases = [
            ("# Hello", "Hello"),
            ("## Sub", "Sub"),
            ("###### Six", "Six"),
            ("####### Seven hashes stays", "####### Seven hashes stays"),
            ("# Title ##", "Title"),
            ("Setext title\n===", "Setext title"),
            ("Setext sub\n---", "Setext sub"),
        ];
        for (i, w) in cases {
            assert_eq!(strip_markdown(i), w, "strip({i:?})");
        }
    }

    #[test]
    fn strip_markdown_inline_formatting() {
        let cases = [
            ("**bold** here", "bold here"),
            ("__bold__ here", "bold here"),
            ("*italic* here", "italic here"),
            ("a _italic_ b", "a italic b"),
            ("~~gone~~", "gone"),
            ("`code` inline", "code inline"),
            ("***boldital***", "boldital"),
            ("nested **b*i*b** case", "nested bib case"),
        ];
        for (i, w) in cases {
            assert_eq!(strip_markdown(i), w, "strip({i:?})");
        }
    }

    #[test]
    fn strip_markdown_links_and_images() {
        let cases = [
            ("[text](https://x.com)", "text"),
            ("click [here](https://y.com) now", "click here now"),
            ("![alt text](img.png)", "alt text"),
            ("[ref][id]", "ref"),
            ("<https://example.com>", "https://example.com"),
            ("<mailto:a@b.c>", "mailto:a@b.c"),
        ];
        for (i, w) in cases {
            assert_eq!(strip_markdown(i), w, "strip({i:?})");
        }
    }

    #[test]
    fn strip_markdown_lists() {
        let input = "- one\n* two\n+ three\n1. four\n2) five";
        assert_eq!(strip_markdown(input), "one\ntwo\nthree\nfour\nfive");
    }

    #[test]
    fn strip_markdown_blockquote() {
        assert_eq!(
            strip_markdown("> quoted line\n>> nested\ntext"),
            "quoted line\nnested\ntext"
        );
    }

    #[test]
    fn strip_markdown_fenced_code_untouched() {
        let input = "```go\nfunc F() {\n\t// **not italic**\n}\n```";
        assert_eq!(strip_markdown(input), "func F() {\n\t// **not italic**\n}");
    }

    #[test]
    fn strip_markdown_horizontal_rule() {
        assert_eq!(
            strip_markdown("before\n---\nafter\n***\nend"),
            "before\nafter\nend"
        );
    }

    #[test]
    fn strip_markdown_table_separator() {
        assert_eq!(
            strip_markdown("| A | B |\n|---|---|\n| 1 | 2 |"),
            "| A | B |\n| 1 | 2 |"
        );
    }

    #[test]
    fn strip_markdown_escaped_chars() {
        assert_eq!(
            strip_markdown(r"\*not italic\* and \[not link\]"),
            "*not italic* and [not link]"
        );
    }

    #[test]
    fn strip_markdown_html_tags() {
        assert_eq!(
            strip_markdown("<b>keep text</b> and <br/> line"),
            "keep text and  line"
        );
    }

    #[test]
    fn export_txt_strips_markdown_while_md_keeps_it() {
        let a = test_app();
        let body = "# Title\n\n**bold** and [link](https://x)";
        a.create_note("", vec![], body).unwrap();
        let dest_md = tempfile::tempdir().unwrap();
        a.export_view(
            dest_md.path().to_str().unwrap(),
            &ExportView {
                kind: ExportViewKind::All,
                tag: String::new(),
            },
            ExportFormat::Md,
        )
        .unwrap();
        let md_files = list_dir(&dest_md.path().join("All"));
        let md_body = fs::read_to_string(dest_md.path().join("All").join(&md_files[0])).unwrap();
        assert_eq!(md_body, body);

        let dest_txt = tempfile::tempdir().unwrap();
        a.export_view(
            dest_txt.path().to_str().unwrap(),
            &ExportView {
                kind: ExportViewKind::All,
                tag: String::new(),
            },
            ExportFormat::Txt,
        )
        .unwrap();
        let txt_files = list_dir(&dest_txt.path().join("All"));
        let txt_body = fs::read_to_string(dest_txt.path().join("All").join(&txt_files[0])).unwrap();
        assert_eq!(txt_body, "Title\n\nbold and link");
    }

    #[test]
    fn import_uses_first_line_as_title() {
        let a = test_app();
        let src = tempfile::tempdir().unwrap();
        let path = src.path().join("a.md");
        fs::write(&path, "# Alpha\nbody").unwrap();
        let ids = a
            .import_files(vec![path.to_string_lossy().to_string()])
            .unwrap();
        let n = a.load_note(&ids[0]).unwrap();
        assert_eq!(n.title, "Alpha");
    }
}
