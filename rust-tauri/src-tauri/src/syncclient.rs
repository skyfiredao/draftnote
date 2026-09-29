use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug)]
pub enum SyncError {
    Conflict,
    CompareUnknownRef,
    Http(String),
    Status(String),
    Json(String),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Conflict => write!(f, "sha mismatch (409)"),
            SyncError::CompareUnknownRef => write!(f, "compare: unknown ref"),
            SyncError::Http(e) => write!(f, "http: {e}"),
            SyncError::Status(e) => write!(f, "{e}"),
            SyncError::Json(e) => write!(f, "json: {e}"),
        }
    }
}

impl std::error::Error for SyncError {}

impl From<reqwest::Error> for SyncError {
    fn from(e: reqwest::Error) -> Self {
        SyncError::Http(e.to_string())
    }
}

impl From<serde_json::Error> for SyncError {
    fn from(e: serde_json::Error) -> Self {
        SyncError::Json(e.to_string())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub sha: String,
}

#[derive(Debug, Deserialize)]
struct FileBody {
    #[serde(default)]
    content: String,
    #[serde(default)]
    sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub sha: String,
    pub author_name: String,
    pub date: String,
    pub subject: String,
}

pub struct SyncClient {
    pub base_url: String,
    pub owner: String,
    pub repo: String,
    pub username: String,
    pub token: String,
    pub use_api: bool,
    pub branch: String,
    http: reqwest::Client,
}

impl SyncClient {
    pub fn new(
        base_url: &str,
        owner: &str,
        repo: &str,
        username: &str,
        token: &str,
        use_api: bool,
    ) -> SyncClient {
        SyncClient {
            base_url: base_url.trim_end_matches('/').to_string(),
            owner: owner.to_string(),
            repo: repo.to_string(),
            username: username.to_string(),
            token: token.to_string(),
            use_api,
            branch: "draftnote".to_string(),
            http: reqwest::Client::builder()
                .user_agent("draftnote")
                .connect_timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("failed to build reqwest client"),
        }
    }

    fn contents_url(&self, path: &str) -> String {
        format!(
            "{}/repos/{}/{}/contents/{}",
            self.base_url, self.owner, self.repo, path
        )
    }

    fn commits_url(&self, r: &str) -> String {
        format!(
            "{}/repos/{}/{}/commits/{}",
            self.base_url, self.owner, self.repo, r
        )
    }

    fn compare_url(&self, base: &str, head: &str) -> String {
        format!(
            "{}/repos/{}/{}/compare/{}...{}",
            self.base_url, self.owner, self.repo, base, head
        )
    }

    fn trees_url(&self) -> String {
        format!(
            "{}/repos/{}/{}/git/trees",
            self.base_url, self.owner, self.repo
        )
    }

    fn git_commits_url(&self) -> String {
        format!(
            "{}/repos/{}/{}/git/commits",
            self.base_url, self.owner, self.repo
        )
    }

    fn refs_url(&self) -> String {
        format!(
            "{}/repos/{}/{}/git/refs",
            self.base_url, self.owner, self.repo
        )
    }

    async fn send(&self, rb: RequestBuilder) -> Result<(StatusCode, Vec<u8>), SyncError> {
        let auth = if self.use_api {
            format!("token {}", self.token)
        } else {
            format!(
                "Basic {}",
                B64.encode(format!("{}:{}", self.username, self.token))
            )
        };
        let resp = rb
            .header("Authorization", auth)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?;
        let status = resp.status();
        let bytes = resp.bytes().await?.to_vec();
        Ok((status, bytes))
    }

    fn get_ref(&self, url: String, r: &str) -> RequestBuilder {
        self.http.request(Method::GET, url).query(&[("ref", r)])
    }

    fn json_req(&self, method: Method, url: String, body: serde_json::Value) -> RequestBuilder {
        self.http
            .request(method, url)
            .header("Content-Type", "application/json")
            .body(serde_json::to_vec(&body).unwrap_or_default())
    }

    pub async fn compare(&self, base: &str, head: &str) -> Result<Vec<FileChange>, SyncError> {
        if base.is_empty() || head.is_empty() || base == head {
            return Ok(Vec::new());
        }
        let (status, body) = self
            .send(self.http.request(Method::GET, self.compare_url(base, head)))
            .await?;
        if status == StatusCode::NOT_FOUND {
            return Err(SyncError::CompareUnknownRef);
        }
        if status != StatusCode::OK {
            return Err(SyncError::Status(format!(
                "compare {base}...{head}: status {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
            )));
        }
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            files: Vec<RawFile>,
        }
        #[derive(Deserialize)]
        struct RawFile {
            #[serde(default)]
            filename: String,
            #[serde(default)]
            status: String,
        }
        let raw: Raw = serde_json::from_slice(&body)?;
        let mut out = Vec::with_capacity(raw.files.len());
        for f in raw.files {
            let status = match f.status.as_str() {
                "added" | "modified" | "removed" => f.status,
                "renamed" => "modified".to_string(),
                "" => "modified".to_string(),
                _ => f.status,
            };
            out.push(FileChange {
                path: f.filename,
                status,
            });
        }
        Ok(out)
    }

    pub async fn head(&self, r: &str) -> Result<String, SyncError> {
        let r = if r.is_empty() {
            self.branch.as_str()
        } else {
            r
        };
        let (status, body) = self
            .send(self.http.request(Method::GET, self.commits_url(r)))
            .await?;
        if status == StatusCode::NOT_FOUND || status == StatusCode::UNPROCESSABLE_ENTITY {
            return Ok(String::new());
        }
        if status != StatusCode::OK {
            return Err(SyncError::Status(format!(
                "head {r}: status {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
            )));
        }
        #[derive(Deserialize)]
        struct CommitRef {
            #[serde(default)]
            sha: String,
        }
        let cr: CommitRef = serde_json::from_slice(&body)?;
        Ok(cr.sha)
    }

    pub async fn list(&self, dir: &str) -> Result<Vec<Entry>, SyncError> {
        let (status, body) = self
            .send(self.get_ref(self.contents_url(dir), &self.branch))
            .await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        if status != StatusCode::OK {
            return Err(SyncError::Status(format!(
                "list {dir}: status {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
            )));
        }
        Ok(serde_json::from_slice(&body)?)
    }

    pub async fn get(&self, path: &str) -> Result<(String, String), SyncError> {
        let (status, body) = self
            .send(self.get_ref(self.contents_url(path), &self.branch))
            .await?;
        self.decode_file(status, body, &format!("get {path}"))
    }

    pub async fn get_at_ref(&self, path: &str, r: &str) -> Result<(String, String), SyncError> {
        let (status, body) = self.send(self.get_ref(self.contents_url(path), r)).await?;
        self.decode_file(status, body, &format!("get at ref {path}"))
    }

    fn decode_file(
        &self,
        status: StatusCode,
        body: Vec<u8>,
        label: &str,
    ) -> Result<(String, String), SyncError> {
        if status == StatusCode::NOT_FOUND {
            return Ok((String::new(), String::new()));
        }
        if status != StatusCode::OK {
            return Err(SyncError::Status(format!(
                "{label}: status {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
            )));
        }
        let f: FileBody = serde_json::from_slice(&body)?;
        let raw = f.content.replace('\n', "");
        let decoded = B64
            .decode(raw)
            .map_err(|e| SyncError::Json(e.to_string()))?;
        let content = String::from_utf8(decoded).map_err(|e| SyncError::Json(e.to_string()))?;
        Ok((content, f.sha))
    }

    pub async fn put(
        &self,
        path: &str,
        content: &str,
        sha: &str,
        message: &str,
    ) -> Result<String, SyncError> {
        let mut body = serde_json::Map::new();
        body.insert("message".into(), json!(message));
        body.insert("content".into(), json!(B64.encode(content.as_bytes())));
        if !sha.is_empty() {
            body.insert("sha".into(), json!(sha));
        }
        if !self.branch.is_empty() {
            body.insert("branch".into(), json!(self.branch));
        }
        let (status, resp_body) = self
            .send(self.json_req(Method::PUT, self.contents_url(path), body.into()))
            .await?;
        if status == StatusCode::CONFLICT {
            return Err(SyncError::Conflict);
        }
        if status != StatusCode::OK && status != StatusCode::CREATED {
            return Err(SyncError::Status(format!(
                "put {path}: status {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&resp_body)
            )));
        }
        #[derive(Deserialize)]
        struct Content {
            #[serde(default)]
            sha: String,
        }
        #[derive(Deserialize)]
        struct PutResp {
            #[serde(default)]
            content: Content,
        }
        impl Default for Content {
            fn default() -> Self {
                Content { sha: String::new() }
            }
        }
        let pr: PutResp = serde_json::from_slice(&resp_body)?;
        Ok(pr.content.sha)
    }

    pub async fn delete(&self, path: &str, sha: &str, message: &str) -> Result<(), SyncError> {
        let mut body = serde_json::Map::new();
        body.insert("message".into(), json!(message));
        body.insert("sha".into(), json!(sha));
        if !self.branch.is_empty() {
            body.insert("branch".into(), json!(self.branch));
        }
        let (status, resp_body) = self
            .send(self.json_req(Method::DELETE, self.contents_url(path), body.into()))
            .await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        if status == StatusCode::CONFLICT {
            return Err(SyncError::Conflict);
        }
        if status != StatusCode::OK && status != StatusCode::NO_CONTENT {
            return Err(SyncError::Status(format!(
                "delete {path}: status {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&resp_body)
            )));
        }
        Ok(())
    }

    pub async fn ensure_branch(&self) -> Result<(), SyncError> {
        if !self.use_api {
            return Ok(());
        }
        let sha = self.head(&self.branch).await?;
        if !sha.is_empty() {
            return Ok(());
        }
        let tree_sha = match self.create_init_tree().await? {
            Some(s) => s,
            None => return Ok(()),
        };
        let commit_sha = self
            .create_orphan_commit(&tree_sha, "init draftnote")
            .await?;
        self.create_branch_ref(&self.branch.clone(), &commit_sha)
            .await
    }

    async fn create_init_tree(&self) -> Result<Option<String>, SyncError> {
        let payload = json!({
            "tree": [{
                "path": "README.md",
                "mode": "100644",
                "type": "blob",
                "content": "# draftnote\n",
            }]
        });
        let (status, body) = self
            .send(self.json_req(Method::POST, self.trees_url(), payload))
            .await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status != StatusCode::CREATED && status != StatusCode::OK {
            return Err(SyncError::Status(format!(
                "create tree: status {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
            )));
        }
        parse_sha(&body).map(Some)
    }

    async fn create_orphan_commit(
        &self,
        tree_sha: &str,
        message: &str,
    ) -> Result<String, SyncError> {
        let payload = json!({
            "message": message,
            "tree": tree_sha,
            "parents": [],
        });
        let (status, body) = self
            .send(self.json_req(Method::POST, self.git_commits_url(), payload))
            .await?;
        if status != StatusCode::CREATED && status != StatusCode::OK {
            return Err(SyncError::Status(format!(
                "create commit: status {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
            )));
        }
        parse_sha(&body)
    }

    async fn create_branch_ref(&self, branch: &str, sha: &str) -> Result<(), SyncError> {
        let payload = json!({
            "ref": format!("refs/heads/{branch}"),
            "sha": sha,
        });
        let (status, body) = self
            .send(self.json_req(Method::POST, self.refs_url(), payload))
            .await?;
        if status == StatusCode::CREATED
            || status == StatusCode::OK
            || status == StatusCode::UNPROCESSABLE_ENTITY
        {
            return Ok(());
        }
        Err(SyncError::Status(format!(
            "create ref {branch}: status {}: {}",
            status.as_u16(),
            String::from_utf8_lossy(&body)
        )))
    }

    pub async fn get_remote(&self) -> Result<String, SyncError> {
        let (status, body) = self
            .send(
                self.http
                    .request(Method::GET, format!("{}/remote", self.base_url)),
            )
            .await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(String::new());
        }
        if status != StatusCode::OK {
            return Err(SyncError::Status(format!(
                "get remote: status {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
            )));
        }
        #[derive(Deserialize)]
        struct Out {
            #[serde(default)]
            url: String,
        }
        let out: Out = serde_json::from_slice(&body)?;
        Ok(out.url)
    }

    pub async fn post_reclone(&self, new_url: &str) -> Result<(), SyncError> {
        let payload = json!({ "url": new_url });
        let (status, body) = self
            .send(self.json_req(Method::POST, format!("{}/remote", self.base_url), payload))
            .await?;
        if status != StatusCode::OK {
            return Err(SyncError::Status(format!(
                "reclone: status {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
            )));
        }
        Ok(())
    }

    pub async fn file_history(
        &self,
        path: &str,
        page: i64,
        per_page: i64,
    ) -> Result<Vec<CommitInfo>, SyncError> {
        let url = format!(
            "{}/repos/{}/{}/commits",
            self.base_url, self.owner, self.repo
        );
        let rb = self.http.request(Method::GET, url).query(&[
            ("path", path.to_string()),
            ("sha", self.branch.clone()),
            ("page", page.to_string()),
            ("per_page", per_page.to_string()),
        ]);
        let (status, body) = self.send(rb).await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        if status != StatusCode::OK {
            return Err(SyncError::Status(format!(
                "history {path}: status {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
            )));
        }
        #[derive(Deserialize)]
        struct Author {
            #[serde(default)]
            name: String,
            #[serde(default)]
            date: String,
        }
        #[derive(Deserialize)]
        struct Commit {
            #[serde(default)]
            message: String,
            #[serde(default)]
            author: Author,
        }
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            sha: String,
            #[serde(default)]
            commit: Commit,
        }
        impl Default for Author {
            fn default() -> Self {
                Author {
                    name: String::new(),
                    date: String::new(),
                }
            }
        }
        impl Default for Commit {
            fn default() -> Self {
                Commit {
                    message: String::new(),
                    author: Author::default(),
                }
            }
        }
        let raw: Vec<Raw> = serde_json::from_slice(&body)?;
        Ok(raw
            .into_iter()
            .map(|r| CommitInfo {
                sha: r.sha,
                author_name: r.commit.author.name,
                date: r.commit.author.date,
                subject: r.commit.message,
            })
            .collect())
    }

    pub async fn commit_patch(&self, sha: &str, path: &str) -> Result<String, SyncError> {
        let (status, body) = self
            .send(self.http.request(Method::GET, self.commits_url(sha)))
            .await?;
        if status != StatusCode::OK {
            return Err(SyncError::Status(format!(
                "commit {sha}: status {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
            )));
        }
        #[derive(Deserialize)]
        struct Patch {
            #[serde(default)]
            filename: String,
            #[serde(default)]
            patch: String,
        }
        #[derive(Deserialize)]
        struct Detail {
            #[serde(default)]
            files: Vec<Patch>,
        }
        let detail: Detail = serde_json::from_slice(&body)?;
        for f in detail.files {
            if f.filename == path {
                return Ok(f.patch);
            }
        }
        Ok(String::new())
    }
}

fn parse_sha(body: &[u8]) -> Result<String, SyncError> {
    #[derive(Deserialize)]
    struct Out {
        #[serde(default)]
        sha: String,
    }
    let out: Out = serde_json::from_slice(body)?;
    Ok(out.sha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[derive(Clone)]
    struct Recorded {
        method: String,
        path: String,
        query: String,
        headers: Vec<(String, String)>,
        body: String,
    }

    struct Mock {
        base: String,
        reqs: Arc<Mutex<Vec<Recorded>>>,
    }

    async fn start_mock<F>(handler: F) -> Mock
    where
        F: Fn(&Recorded) -> (u16, String) + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let reqs = Arc::new(Mutex::new(Vec::<Recorded>::new()));
        let reqs2 = reqs.clone();
        let handler = Arc::new(handler);
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let reqs = reqs2.clone();
                let handler = handler.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 1024];
                    let (head_end, content_len) = loop {
                        let n = match sock.read(&mut tmp).await {
                            Ok(0) => return,
                            Ok(n) => n,
                            Err(_) => return,
                        };
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..pos]).to_string();
                            let cl = parse_content_length(&head);
                            break (pos + 4, cl);
                        }
                    };
                    while buf.len() < head_end + content_len {
                        let n = match sock.read(&mut tmp).await {
                            Ok(0) => break,
                            Ok(n) => n,
                            Err(_) => break,
                        };
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let head = String::from_utf8_lossy(&buf[..head_end - 4]).to_string();
                    let body =
                        String::from_utf8_lossy(&buf[head_end..head_end + content_len]).to_string();
                    let rec = parse_request(&head, body);
                    let (code, resp_body) = handler(&rec);
                    reqs.lock().unwrap().push(rec);
                    let resp = format!(
                        "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        code,
                        resp_body.as_bytes().len(),
                        resp_body
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.flush().await;
                });
            }
        });
        Mock {
            base: format!("http://{addr}"),
            reqs,
        }
    }

    fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    fn parse_content_length(head: &str) -> usize {
        for line in head.split("\r\n") {
            if let Some(v) = line.strip_prefix("Content-Length: ") {
                return v.trim().parse().unwrap_or(0);
            }
            if let Some(v) = line.strip_prefix("content-length: ") {
                return v.trim().parse().unwrap_or(0);
            }
        }
        0
    }

    fn parse_request(head: &str, body: String) -> Recorded {
        let mut lines = head.split("\r\n");
        let first = lines.next().unwrap_or("");
        let mut parts = first.split(' ');
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("").to_string();
        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (p.to_string(), q.to_string()),
            None => (target, String::new()),
        };
        let mut headers = Vec::new();
        for line in lines {
            if let Some((k, v)) = line.split_once(": ") {
                headers.push((k.to_string(), v.to_string()));
            }
        }
        Recorded {
            method,
            path,
            query,
            headers,
            body,
        }
    }

    fn client(base: &str) -> SyncClient {
        SyncClient::new(base, "owner", "repo", "", "tok", true)
    }

    fn header<'a>(rec: &'a Recorded, key: &str) -> Option<&'a str> {
        rec.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    #[tokio::test]
    async fn list_returns_entries() {
        let m = start_mock(|rec| {
            assert_eq!(header(rec, "authorization"), Some("token tok"));
            (
                200,
                r#"[{"name":"a.json","path":"notes/a.json","type":"file","sha":"sha-a"},
                    {"name":"b.json","path":"notes/b.json","type":"file","sha":"sha-b"}]"#
                    .to_string(),
            )
        })
        .await;
        let entries = client(&m.base).list("notes").await.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].sha, "sha-a");
    }

    #[tokio::test]
    async fn list_not_found_is_empty() {
        let m = start_mock(|_| (404, String::new())).await;
        let entries = client(&m.base).list("notes").await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn get_decodes_base64() {
        let payload = "# 正文\n内容";
        let enc = B64.encode(payload.as_bytes());
        let m = start_mock(move |_| (200, format!(r#"{{"content":"{enc}","sha":"sha1"}}"#))).await;
        let (content, sha) = client(&m.base).get("notes/a.json").await.unwrap();
        assert_eq!(content, payload);
        assert_eq!(sha, "sha1");
    }

    #[tokio::test]
    async fn get_not_found() {
        let m = start_mock(|_| (404, String::new())).await;
        let (content, sha) = client(&m.base).get("notes/x.json").await.unwrap();
        assert_eq!(content, "");
        assert_eq!(sha, "");
    }

    #[tokio::test]
    async fn put_sends_sha_and_encodes_content() {
        let reqs = {
            let m = start_mock(|rec| {
                assert_eq!(rec.method, "PUT");
                let pb: serde_json::Value = serde_json::from_str(&rec.body).unwrap();
                assert_eq!(pb["sha"], "old-sha");
                let decoded = B64.decode(pb["content"].as_str().unwrap()).unwrap();
                assert_eq!(String::from_utf8(decoded).unwrap(), "hello world");
                (200, r#"{"content":{"sha":"new-sha"}}"#.to_string())
            })
            .await;
            let new_sha = client(&m.base)
                .put("notes/a.json", "hello world", "old-sha", "msg")
                .await
                .unwrap();
            assert_eq!(new_sha, "new-sha");
            m.reqs
        };
        assert_eq!(reqs.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn put_conflict_returns_err_conflict() {
        let m = start_mock(|_| (409, String::new())).await;
        let err = client(&m.base)
            .put("notes/a.json", "x", "stale", "msg")
            .await
            .unwrap_err();
        assert!(matches!(err, SyncError::Conflict));
    }

    #[tokio::test]
    async fn put_server_error() {
        let m = start_mock(|_| (500, "boom".to_string())).await;
        let err = client(&m.base)
            .put("notes/a.json", "x", "", "msg")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("500"));
    }

    #[tokio::test]
    async fn head_returns_sha() {
        let m = start_mock(|rec| {
            assert!(rec.path.ends_with("/commits/draftnote"));
            (200, r#"{"sha":"abc123","other":"ignored"}"#.to_string())
        })
        .await;
        let sha = client(&m.base).head("").await.unwrap();
        assert_eq!(sha, "abc123");
    }

    #[tokio::test]
    async fn head_empty_on_not_found() {
        let m = start_mock(|_| (404, String::new())).await;
        assert_eq!(client(&m.base).head("").await.unwrap(), "");
    }

    #[tokio::test]
    async fn head_empty_on_unprocessable() {
        let m = start_mock(|_| (422, r#"{"message":"no commit"}"#.to_string())).await;
        assert_eq!(client(&m.base).head("").await.unwrap(), "");
    }

    #[tokio::test]
    async fn ensure_branch_noop_when_data_api_missing() {
        let m = start_mock(|rec| {
            if rec.path.contains("/commits/draftnote") {
                return (404, String::new());
            }
            if rec.method == "POST" && rec.path == "/repos/owner/repo/git/trees" {
                return (404, String::new());
            }
            (500, format!("unexpected {} {}", rec.method, rec.path))
        })
        .await;
        client(&m.base).ensure_branch().await.unwrap();
        let reqs = m.reqs.lock().unwrap();
        let tree_posts = reqs
            .iter()
            .filter(|r| r.method == "POST" && r.path == "/repos/owner/repo/git/trees")
            .count();
        assert_eq!(tree_posts, 1);
    }

    #[tokio::test]
    async fn ensure_branch_noop_for_self_hosted() {
        let m = start_mock(|_| (200, String::new())).await;
        let mut c = client(&m.base);
        c.use_api = false;
        c.ensure_branch().await.unwrap();
        assert_eq!(m.reqs.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn ensure_branch_noop_when_exists() {
        let m = start_mock(|rec| {
            assert!(rec.path.contains("/commits/draftnote"));
            (200, r#"{"sha":"deadbeef"}"#.to_string())
        })
        .await;
        client(&m.base).ensure_branch().await.unwrap();
        assert_eq!(m.reqs.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn ensure_branch_creates_orphan() {
        let m = start_mock(|rec| match rec.path.as_str() {
            "/repos/owner/repo/commits/draftnote" => (404, String::new()),
            "/repos/owner/repo/git/trees" => (201, r#"{"sha":"treesha"}"#.to_string()),
            "/repos/owner/repo/git/commits" => (201, r#"{"sha":"commitsha"}"#.to_string()),
            "/repos/owner/repo/git/refs" => (201, "{}".to_string()),
            _ => (500, format!("unexpected {}", rec.path)),
        })
        .await;
        client(&m.base).ensure_branch().await.unwrap();
        let reqs = m.reqs.lock().unwrap();
        let tree = reqs
            .iter()
            .find(|r| r.path.ends_with("/git/trees"))
            .unwrap();
        assert!(tree.body.contains(r#""path""#) && tree.body.contains("README.md"));
        let commit = reqs
            .iter()
            .find(|r| r.path.ends_with("/git/commits"))
            .unwrap();
        assert!(commit.body.contains(r#""parents":[]"#));
        assert!(commit.body.contains(r#""tree":"treesha""#));
        let rf = reqs.iter().find(|r| r.path.ends_with("/git/refs")).unwrap();
        assert!(rf.body.contains(r#""ref":"refs/heads/draftnote""#));
        assert!(rf.body.contains(r#""sha":"commitsha""#));
    }

    #[tokio::test]
    async fn compare_parses_files() {
        let m = start_mock(|rec| {
            assert!(rec.path.contains("/compare/base-sha...head-sha"));
            (
                200,
                r#"{"files":[{"filename":"notes/a.json","status":"added"},{"filename":"notes/b.json","status":"modified"},{"filename":"notes/c.json","status":"removed"},{"filename":"notes/d.json","status":"renamed"}]}"#
                    .to_string(),
            )
        })
        .await;
        let got = client(&m.base)
            .compare("base-sha", "head-sha")
            .await
            .unwrap();
        assert_eq!(got.len(), 4);
        assert_eq!(got[0].path, "notes/a.json");
        assert_eq!(got[0].status, "added");
        assert_eq!(got[3].status, "modified");
    }

    #[tokio::test]
    async fn compare_equal_bases_short_circuits() {
        let m = start_mock(|_| (200, String::new())).await;
        let got = client(&m.base).compare("same", "same").await.unwrap();
        assert!(got.is_empty());
        assert_eq!(m.reqs.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn get_sends_branch_ref() {
        let m = start_mock(|_| (200, r#"{"content":"","sha":""}"#.to_string())).await;
        let c = client(&m.base);
        let _ = c.get("notes/x.json").await;
        let reqs = m.reqs.lock().unwrap();
        assert!(reqs[0].query.contains("ref=draftnote"));
    }

    #[tokio::test]
    async fn list_sends_branch_ref() {
        let m = start_mock(|_| (200, "[]".to_string())).await;
        let _ = client(&m.base).list("notes").await;
        let reqs = m.reqs.lock().unwrap();
        assert!(reqs[0].query.contains("ref=draftnote"));
    }

    #[tokio::test]
    async fn sends_user_agent_header() {
        let m = start_mock(|_| (200, "[]".to_string())).await;
        let _ = client(&m.base).list("notes").await;
        let reqs = m.reqs.lock().unwrap();
        let ua = reqs[0]
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
            .map(|(_, v)| v.clone());
        assert_eq!(ua.as_deref(), Some("draftnote"));
    }
}
