package syncclient

import (
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func testClient(h http.Handler) (*Client, *httptest.Server) {
	srv := httptest.NewServer(h)
	c := New(srv.URL, "owner", "repo", "", "tok", true)
	return c, srv
}

func TestListReturnsEntries(t *testing.T) {
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "token tok" {
			t.Errorf("missing auth header: %q", r.Header.Get("Authorization"))
		}
		json.NewEncoder(w).Encode([]Entry{
			{Name: "a.json", Path: "notes/a.json", Type: "file", SHA: "sha-a"},
			{Name: "b.json", Path: "notes/b.json", Type: "file", SHA: "sha-b"},
		})
	}))
	defer srv.Close()

	entries, err := c.List("notes")
	if err != nil {
		t.Fatalf("list: %v", err)
	}
	if len(entries) != 2 || entries[0].SHA != "sha-a" {
		t.Errorf("entries = %+v", entries)
	}
}

func TestListNotFoundIsEmpty(t *testing.T) {
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusNotFound)
	}))
	defer srv.Close()
	entries, err := c.List("notes")
	if err != nil || entries != nil {
		t.Errorf("expected empty, got %v %v", entries, err)
	}
}

func TestGetDecodesBase64(t *testing.T) {
	payload := "# 正文\n内容"
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(File{
			Content: base64.StdEncoding.EncodeToString([]byte(payload)),
			SHA:     "sha1",
		})
	}))
	defer srv.Close()

	content, sha, err := c.Get("notes/a.json")
	if err != nil {
		t.Fatalf("get: %v", err)
	}
	if content != payload || sha != "sha1" {
		t.Errorf("content=%q sha=%q", content, sha)
	}
}

func TestGetNotFound(t *testing.T) {
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusNotFound)
	}))
	defer srv.Close()
	content, sha, err := c.Get("notes/x.json")
	if err != nil || content != "" || sha != "" {
		t.Errorf("expected empty, got %q %q %v", content, sha, err)
	}
}

func TestPutSendsShaAndEncodesContent(t *testing.T) {
	payload := "hello world"
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPut {
			t.Errorf("method = %s", r.Method)
		}
		b, _ := io.ReadAll(r.Body)
		var pb putBody
		json.Unmarshal(b, &pb)
		if pb.SHA != "old-sha" {
			t.Errorf("sha not sent: %q", pb.SHA)
		}
		decoded, _ := base64.StdEncoding.DecodeString(pb.Content)
		if string(decoded) != payload {
			t.Errorf("content = %q", decoded)
		}
		resp := putResponse{}
		resp.Content.SHA = "new-sha"
		json.NewEncoder(w).Encode(resp)
	}))
	defer srv.Close()

	newSHA, err := c.Put("notes/a.json", payload, "old-sha", "msg")
	if err != nil {
		t.Fatalf("put: %v", err)
	}
	if newSHA != "new-sha" {
		t.Errorf("newSHA = %q", newSHA)
	}
}

func TestPutConflictReturnsErrConflict(t *testing.T) {
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusConflict)
	}))
	defer srv.Close()
	_, err := c.Put("notes/a.json", "x", "stale", "msg")
	if err != ErrConflict {
		t.Errorf("expected ErrConflict, got %v", err)
	}
}

func TestPutServerError(t *testing.T) {
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
		io.WriteString(w, "boom")
	}))
	defer srv.Close()
	_, err := c.Put("notes/a.json", "x", "", "msg")
	if err == nil || !strings.Contains(err.Error(), "500") {
		t.Errorf("expected 500 error, got %v", err)
	}
}

func TestHeadReturnsSHA(t *testing.T) {
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !strings.HasSuffix(r.URL.Path, "/commits/draftnote") {
			t.Errorf("unexpected path: %s", r.URL.Path)
		}
		w.Write([]byte(`{"sha":"abc123","other":"ignored"}`))
	}))
	defer srv.Close()
	sha, err := c.Head("")
	if err != nil {
		t.Fatalf("head: %v", err)
	}
	if sha != "abc123" {
		t.Errorf("sha = %q", sha)
	}
}

func TestHeadReturnsEmptyOnNotFound(t *testing.T) {
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusNotFound)
	}))
	defer srv.Close()
	sha, err := c.Head("")
	if err != nil {
		t.Fatalf("head: %v", err)
	}
	if sha != "" {
		t.Errorf("expected empty sha on 404, got %q", sha)
	}
}

func TestHeadReturnsEmptyOnUnprocessableEntity(t *testing.T) {
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusUnprocessableEntity)
		w.Write([]byte(`{"message":"No commit found for SHA: draftnote"}`))
	}))
	defer srv.Close()
	sha, err := c.Head("")
	if err != nil {
		t.Fatalf("head: %v", err)
	}
	if sha != "" {
		t.Errorf("expected empty sha on 422, got %q", sha)
	}
}

func TestEnsureBranchNoOpWhenDataAPIMissing(t *testing.T) {
	posts := 0
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.Contains(r.URL.Path, "/commits/draftnote") {
			w.WriteHeader(http.StatusNotFound)
			return
		}
		if r.Method == http.MethodPost && r.URL.Path == "/repos/owner/repo/git/trees" {
			posts++
			w.WriteHeader(http.StatusNotFound)
			return
		}
		t.Errorf("unexpected %s %s", r.Method, r.URL.Path)
	}))
	defer srv.Close()
	if err := c.EnsureBranch(); err != nil {
		t.Fatalf("ensure should degrade gracefully: %v", err)
	}
	if posts != 1 {
		t.Errorf("expected 1 tree POST attempt, got %d", posts)
	}
}

func TestEnsureBranchNoOpForSelfHosted(t *testing.T) {
	calls := 0
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls++
	}))
	defer srv.Close()
	c.UseAPI = false
	if err := c.EnsureBranch(); err != nil {
		t.Fatalf("ensure: %v", err)
	}
	if calls != 0 {
		t.Errorf("expected 0 HTTP calls in self-hosted mode, got %d", calls)
	}
}

func TestEnsureBranchNoOpWhenExists(t *testing.T) {
	heads := 0
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.Contains(r.URL.Path, "/commits/draftnote") {
			heads++
			w.Write([]byte(`{"sha":"deadbeef"}`))
			return
		}
		t.Errorf("unexpected path %s", r.URL.Path)
	}))
	defer srv.Close()
	if err := c.EnsureBranch(); err != nil {
		t.Fatalf("ensure: %v", err)
	}
	if heads != 1 {
		t.Errorf("expected 1 head call, got %d", heads)
	}
}

func TestEnsureBranchCreatesOrphan(t *testing.T) {
	var seen struct {
		draftnoteHead int
		treePost      int
		commitPost    int
		refsPost      int
		treeBody      string
		commitBody    string
		refBody       string
	}
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case r.URL.Path == "/repos/owner/repo/commits/draftnote":
			seen.draftnoteHead++
			w.WriteHeader(http.StatusNotFound)
		case r.Method == http.MethodPost && r.URL.Path == "/repos/owner/repo/git/trees":
			seen.treePost++
			body, _ := io.ReadAll(r.Body)
			seen.treeBody = string(body)
			w.WriteHeader(http.StatusCreated)
			w.Write([]byte(`{"sha":"treesha"}`))
		case r.Method == http.MethodPost && r.URL.Path == "/repos/owner/repo/git/commits":
			seen.commitPost++
			body, _ := io.ReadAll(r.Body)
			seen.commitBody = string(body)
			w.WriteHeader(http.StatusCreated)
			w.Write([]byte(`{"sha":"commitsha"}`))
		case r.Method == http.MethodPost && r.URL.Path == "/repos/owner/repo/git/refs":
			seen.refsPost++
			body, _ := io.ReadAll(r.Body)
			seen.refBody = string(body)
			w.WriteHeader(http.StatusCreated)
			w.Write([]byte(`{}`))
		default:
			t.Errorf("unexpected %s %s", r.Method, r.URL.Path)
		}
	}))
	defer srv.Close()
	if err := c.EnsureBranch(); err != nil {
		t.Fatalf("ensure: %v", err)
	}
	if seen.draftnoteHead != 1 || seen.treePost != 1 || seen.commitPost != 1 || seen.refsPost != 1 {
		t.Fatalf("call counts wrong: %+v", seen)
	}
	if !strings.Contains(seen.treeBody, `"path":"README.md"`) {
		t.Errorf("tree body missing README.md placeholder: %s", seen.treeBody)
	}
	if !strings.Contains(seen.commitBody, `"parents":[]`) {
		t.Errorf("commit body must have empty parents (orphan): %s", seen.commitBody)
	}
	if !strings.Contains(seen.commitBody, `"tree":"treesha"`) {
		t.Errorf("commit body missing tree sha: %s", seen.commitBody)
	}
	if !strings.Contains(seen.refBody, `"ref":"refs/heads/draftnote"`) {
		t.Errorf("ref body missing ref: %s", seen.refBody)
	}
	if !strings.Contains(seen.refBody, `"sha":"commitsha"`) {
		t.Errorf("ref body missing commit sha: %s", seen.refBody)
	}
}

func TestCompareParsesFiles(t *testing.T) {
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !strings.Contains(r.URL.Path, "/compare/base-sha...head-sha") {
			t.Errorf("path = %s", r.URL.Path)
		}
		w.Write([]byte(`{"files":[{"filename":"notes/a.json","status":"added"},{"filename":"notes/b.json","status":"modified"},{"filename":"notes/c.json","status":"removed"},{"filename":"notes/d.json","status":"renamed"}]}`))
	}))
	defer srv.Close()
	got, err := c.Compare("base-sha", "head-sha")
	if err != nil {
		t.Fatalf("compare: %v", err)
	}
	if len(got) != 4 {
		t.Fatalf("len=%d", len(got))
	}
	if got[0].Path != "notes/a.json" || got[0].Status != "added" {
		t.Errorf("[0]=%+v", got[0])
	}
	if got[3].Status != "modified" {
		t.Errorf("renamed should map to modified, got %s", got[3].Status)
	}
}

func TestCompareEqualBasesShortCircuits(t *testing.T) {
	called := false
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		called = true
	}))
	defer srv.Close()
	got, err := c.Compare("same", "same")
	if err != nil {
		t.Fatalf("compare: %v", err)
	}
	if len(got) != 0 || called {
		t.Errorf("expected shortcut, got %d changes, called=%v", len(got), called)
	}
}

func TestGetSendsBranchRef(t *testing.T) {
	var gotRef string
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotRef = r.URL.Query().Get("ref")
		w.Write([]byte(`{"content":"","sha":""}`))
	}))
	defer srv.Close()
	_, _, _ = c.Get("notes/x.json")
	if gotRef != "draftnote" {
		t.Errorf("expected ref=draftnote, got %q", gotRef)
	}
}

func TestListSendsBranchRef(t *testing.T) {
	var gotRef string
	c, srv := testClient(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotRef = r.URL.Query().Get("ref")
		w.Write([]byte(`[]`))
	}))
	defer srv.Close()
	_, _ = c.List("notes")
	if gotRef != "draftnote" {
		t.Errorf("expected ref=draftnote, got %q", gotRef)
	}
}
