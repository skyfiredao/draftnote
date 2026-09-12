const MARKER_LOCAL: &str = "<<<<<<< local version";
const MARKER_SEP: &str = "=======";
const MARKER_REMOTE: &str = ">>>>>>> remote version";

struct Hunk {
    equal: bool,
    local: Vec<String>,
    remote: Vec<String>,
}

pub fn merge(local: &str, remote: &str) -> String {
    if local == remote {
        return local.to_string();
    }
    let local_lines = split_lines(local);
    let remote_lines = split_lines(remote);
    let hunks = diff(&local_lines, &remote_lines);

    let mut out: Vec<String> = Vec::new();
    for h in &hunks {
        if h.equal {
            out.extend(h.local.iter().cloned());
        } else if h.local.is_empty() {
            out.extend(h.remote.iter().cloned());
        } else if h.remote.is_empty() {
            out.extend(h.local.iter().cloned());
        } else {
            out.push(MARKER_LOCAL.to_string());
            out.extend(h.local.iter().cloned());
            out.push(MARKER_SEP.to_string());
            out.extend(h.remote.iter().cloned());
            out.push(MARKER_REMOTE.to_string());
        }
    }
    join_lines(&out, local, remote)
}

pub fn has_markers(text: &str) -> bool {
    text.split('\n')
        .any(|line| line == MARKER_LOCAL || line == MARKER_SEP || line == MARKER_REMOTE)
}

fn diff(a: &[String], b: &[String]) -> Vec<Hunk> {
    let lcs = lcs_matrix(a, b);
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let mut pending_a: Vec<String> = Vec::new();
    let mut pending_b: Vec<String> = Vec::new();

    macro_rules! flush {
        () => {
            if !pending_a.is_empty() || !pending_b.is_empty() {
                hunks.push(Hunk {
                    equal: false,
                    local: std::mem::take(&mut pending_a),
                    remote: std::mem::take(&mut pending_b),
                });
            }
        };
    }

    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            flush!();
            hunks.push(Hunk {
                equal: true,
                local: vec![a[i].clone()],
                remote: Vec::new(),
            });
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            pending_a.push(a[i].clone());
            i += 1;
        } else {
            pending_b.push(b[j].clone());
            j += 1;
        }
    }
    while i < a.len() {
        pending_a.push(a[i].clone());
        i += 1;
    }
    while j < b.len() {
        pending_b.push(b[j].clone());
        j += 1;
    }
    flush!();
    merge_adjacent(hunks)
}

fn merge_adjacent(hunks: Vec<Hunk>) -> Vec<Hunk> {
    let mut out: Vec<Hunk> = Vec::new();
    for h in hunks {
        if h.equal {
            if let Some(last) = out.last_mut() {
                if last.equal {
                    last.local.extend(h.local);
                    continue;
                }
            }
        }
        out.push(h);
    }
    out
}

fn lcs_matrix(a: &[String], b: &[String]) -> Vec<Vec<usize>> {
    let (m, n) = (a.len(), b.len());
    let mut lcs = vec![vec![0usize; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            if a[i] == b[j] {
                lcs[i][j] = lcs[i + 1][j + 1] + 1;
            } else if lcs[i + 1][j] >= lcs[i][j + 1] {
                lcs[i][j] = lcs[i + 1][j];
            } else {
                lcs[i][j] = lcs[i][j + 1];
            }
        }
    }
    lcs
}

fn split_lines(s: &str) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    s.strip_suffix('\n')
        .unwrap_or(s)
        .split('\n')
        .map(|l| l.to_string())
        .collect()
}

fn join_lines(lines: &[String], local: &str, remote: &str) -> String {
    let mut joined = lines.join("\n");
    if local.ends_with('\n') || remote.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical() {
        let s = "line1\nline2\n";
        assert_eq!(merge(s, s), s);
    }

    #[test]
    fn pure_addition_remote() {
        let got = merge("line1\nline2\n", "line1\nline2\nline3\n");
        assert!(!has_markers(&got));
        assert!(got.contains("line3"));
    }

    #[test]
    fn pure_addition_local() {
        let got = merge("line1\nline2\nlineX\n", "line1\nline2\n");
        assert!(!has_markers(&got));
        assert!(got.contains("lineX"));
    }

    #[test]
    fn same_line_conflict() {
        let got = merge("line1\nAAA\nline3\n", "line1\nBBB\nline3\n");
        assert!(has_markers(&got));
        for m in [MARKER_LOCAL, MARKER_SEP, MARKER_REMOTE] {
            assert!(got.contains(m), "marker {m} missing in {got:?}");
        }
        let li = got.find("AAA").unwrap();
        let ri = got.find("BBB").unwrap();
        assert!(li < ri, "local should precede remote: {got:?}");
        assert!(got.contains("line1") && got.contains("line3"));
    }

    #[test]
    fn marker_lines_standalone() {
        let got = merge("x\nAAA\n", "x\nBBB\n");
        let found = got
            .strip_suffix('\n')
            .unwrap_or(&got)
            .split('\n')
            .any(|l| l == MARKER_LOCAL);
        assert!(found, "marker not standalone line: {got:?}");
    }

    #[test]
    fn multiple_conflict_segments() {
        let got = merge("a\nX1\nb\nY1\nc\n", "a\nX2\nb\nY2\nc\n");
        assert_eq!(got.matches(MARKER_LOCAL).count(), 2, "got: {got:?}");
    }

    #[test]
    fn chinese_content() {
        let got = merge("标题\n本地内容\n结尾\n", "标题\n服务器内容\n结尾\n");
        assert!(has_markers(&got));
        assert!(got.contains("本地内容") && got.contains("服务器内容"));
    }

    #[test]
    fn empty_local() {
        let got = merge("", "remote\n");
        assert!(!has_markers(&got));
        assert!(got.contains("remote"));
    }

    #[test]
    fn empty_remote() {
        let got = merge("local\n", "");
        assert!(!has_markers(&got));
        assert!(got.contains("local"));
    }

    #[test]
    fn conflict_at_eof() {
        let got = merge("line1\nAAA\n", "line1\nBBB\n");
        assert!(has_markers(&got));
    }

    #[test]
    fn has_markers_detection() {
        assert!(!has_markers("clean\ntext\n"));
        assert!(has_markers(&format!("a\n{MARKER_LOCAL}\nb\n")));
        assert!(!has_markers("this <<<<<<< inline not a marker\n"));
    }
}
