package server

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"draftnote-backend/gitstore"
)

func newServer(t *testing.T) *httptest.Server {
	t.Helper()
	store, err := gitstore.Open(t.TempDir())
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	s := &Server{Store: store, Username: "alice", Token: "tok"}
	return httptest.NewServer(s.Handler())
}

func put(t *testing.T, base, path, content, sha string) *http.Response {
	t.Helper()
	body, _ := json.Marshal(putRequest{
		Message: "m",
		Content: base64.StdEncoding.EncodeToString([]byte(content)),
		SHA:     sha,
	})
	req, _ := http.NewRequest(http.MethodPut, base+"/repos/o/r/contents/"+path, bytes.NewReader(body))
	req.Header.Set("Authorization", "token tok")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("put: %v", err)
	}
	return resp
}

func get(t *testing.T, base, path string) *http.Response {
	t.Helper()
	req, _ := http.NewRequest(http.MethodGet, base+"/repos/o/r/contents/"+path, nil)
	req.Header.Set("Authorization", "token tok")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("get: %v", err)
	}
	return resp
}

func TestAuthRequired(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()
	req, _ := http.NewRequest(http.MethodGet, srv.URL+"/repos/o/r/contents/notes/a.json", nil)
	resp, _ := http.DefaultClient.Do(req)
	if resp.StatusCode != http.StatusUnauthorized {
		t.Errorf("expected 401, got %d", resp.StatusCode)
	}
}

func TestAuthBasicAcceptedWhenCredsMatch(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()
	req, _ := http.NewRequest(http.MethodGet, srv.URL+"/repos/o/r/contents/notes", nil)
	req.SetBasicAuth("alice", "tok")
	resp, _ := http.DefaultClient.Do(req)
	if resp.StatusCode == http.StatusUnauthorized {
		t.Errorf("basic auth with correct creds should not be rejected, got 401")
	}
}

func TestAuthBasicRejectedOnWrongUsername(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()
	req, _ := http.NewRequest(http.MethodGet, srv.URL+"/repos/o/r/contents/notes", nil)
	req.SetBasicAuth("bob", "tok")
	resp, _ := http.DefaultClient.Do(req)
	if resp.StatusCode != http.StatusUnauthorized {
		t.Errorf("expected 401, got %d", resp.StatusCode)
	}
}

func TestAuthBasicRejectedOnWrongPassword(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()
	req, _ := http.NewRequest(http.MethodGet, srv.URL+"/repos/o/r/contents/notes", nil)
	req.SetBasicAuth("alice", "wrong")
	resp, _ := http.DefaultClient.Do(req)
	if resp.StatusCode != http.StatusUnauthorized {
		t.Errorf("expected 401, got %d", resp.StatusCode)
	}
}

func TestCreateThenGet(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()

	resp := put(t, srv.URL, "notes/a.json", `{"id":"a"}`, "")
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("create status %d", resp.StatusCode)
	}
	var pr putResponse
	json.NewDecoder(resp.Body).Decode(&pr)
	if pr.Content.SHA == "" {
		t.Fatal("no sha returned")
	}

	g := get(t, srv.URL, "notes/a.json")
	if g.StatusCode != http.StatusOK {
		t.Fatalf("get status %d", g.StatusCode)
	}
	var f fileResponse
	json.NewDecoder(g.Body).Decode(&f)
	decoded, _ := base64.StdEncoding.DecodeString(f.Content)
	if string(decoded) != `{"id":"a"}` {
		t.Errorf("content = %q", decoded)
	}
	if f.SHA != pr.Content.SHA {
		t.Errorf("sha mismatch: get %q put %q", f.SHA, pr.Content.SHA)
	}
}

func TestUpdateWithCorrectSHA(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()

	r1 := put(t, srv.URL, "notes/a.json", "v1", "")
	var pr1 putResponse
	json.NewDecoder(r1.Body).Decode(&pr1)

	r2 := put(t, srv.URL, "notes/a.json", "v2", pr1.Content.SHA)
	if r2.StatusCode != http.StatusOK {
		t.Fatalf("update status %d", r2.StatusCode)
	}
	g := get(t, srv.URL, "notes/a.json")
	var f fileResponse
	json.NewDecoder(g.Body).Decode(&f)
	decoded, _ := base64.StdEncoding.DecodeString(f.Content)
	if string(decoded) != "v2" {
		t.Errorf("content = %q", decoded)
	}
}

func TestUpdateWithStaleSHAConflicts(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()

	put(t, srv.URL, "notes/a.json", "v1", "")
	resp := put(t, srv.URL, "notes/a.json", "v2", "stale-sha")
	if resp.StatusCode != http.StatusConflict {
		t.Errorf("expected 409, got %d", resp.StatusCode)
	}
}

func TestListDirectory(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()
	put(t, srv.URL, "notes/a.json", "a", "")
	put(t, srv.URL, "notes/b.json", "b", "")

	g := get(t, srv.URL, "notes")
	if g.StatusCode != http.StatusOK {
		t.Fatalf("list status %d", g.StatusCode)
	}
	var entries []fileResponse
	json.NewDecoder(g.Body).Decode(&entries)
	if len(entries) != 2 {
		t.Errorf("expected 2 entries, got %d", len(entries))
	}
}

func TestCommitsHeadEndpointReturnsSHA(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()

	resp := put(t, srv.URL, "notes/20250101-120000-abcd.json", `{"id":"20250101-120000-abcd"}`, "")
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("seed put: %d", resp.StatusCode)
	}

	req, _ := http.NewRequest(http.MethodGet, srv.URL+"/repos/o/r/commits/main", nil)
	req.Header.Set("Authorization", "token tok")
	r, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("get commits: %v", err)
	}
	defer r.Body.Close()
	if r.StatusCode != http.StatusOK {
		t.Fatalf("commits status = %d", r.StatusCode)
	}
	var body struct {
		SHA string `json:"sha"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if body.SHA == "" {
		t.Errorf("expected non-empty sha")
	}
}

func TestCommitsHeadEndpointEmptyRepoReturnsEmptySHA(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()
	req, _ := http.NewRequest(http.MethodGet, srv.URL+"/repos/o/r/commits/main", nil)
	req.Header.Set("Authorization", "token tok")
	r, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("get commits: %v", err)
	}
	defer r.Body.Close()
	if r.StatusCode != http.StatusOK {
		t.Fatalf("commits status = %d", r.StatusCode)
	}
	var body struct {
		SHA string `json:"sha"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		t.Fatalf("decode: %v", err)
	}
	if body.SHA != "" {
		t.Errorf("expected empty sha on empty repo, got %q", body.SHA)
	}
}

func TestDeleteRemovesNoteAndSecondGetIs404(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()
	r := put(t, srv.URL, "notes/20250101-120000-abcd.json", `{"id":"20250101-120000-abcd"}`, "")
	if r.StatusCode != http.StatusOK {
		t.Fatalf("seed put: %d", r.StatusCode)
	}
	var pr putResponse
	json.NewDecoder(r.Body).Decode(&pr)

	body, _ := json.Marshal(deleteRequest{Message: "gone", SHA: pr.Content.SHA})
	req, _ := http.NewRequest(http.MethodDelete, srv.URL+"/repos/o/r/contents/notes/20250101-120000-abcd.json", bytes.NewReader(body))
	req.Header.Set("Authorization", "token tok")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("delete: %v", err)
	}
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("delete status = %d", resp.StatusCode)
	}
	g := get(t, srv.URL, "notes/20250101-120000-abcd.json")
	if g.StatusCode != http.StatusNotFound {
		t.Errorf("expected 404 after delete, got %d", g.StatusCode)
	}
}

func TestDeleteWithStaleSHAReturns409(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()
	put(t, srv.URL, "notes/20250101-120000-abcd.json", "seed", "")

	body, _ := json.Marshal(deleteRequest{Message: "x", SHA: "stale"})
	req, _ := http.NewRequest(http.MethodDelete, srv.URL+"/repos/o/r/contents/notes/20250101-120000-abcd.json", bytes.NewReader(body))
	req.Header.Set("Authorization", "token tok")
	resp, _ := http.DefaultClient.Do(req)
	if resp.StatusCode != http.StatusConflict {
		t.Errorf("expected 409, got %d", resp.StatusCode)
	}
}

func TestRemoteGetReturnsEmptyForInitOnly(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()
	req, _ := http.NewRequest(http.MethodGet, srv.URL+"/remote", nil)
	req.Header.Set("Authorization", "token tok")
	r, _ := http.DefaultClient.Do(req)
	if r.StatusCode != http.StatusOK {
		t.Fatalf("status = %d", r.StatusCode)
	}
	var body struct {
		URL string `json:"url"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		t.Fatal(err)
	}
	if body.URL != "" {
		t.Errorf("expected empty url, got %q", body.URL)
	}
}

func TestRemotePostRejectsEmpty(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()
	req, _ := http.NewRequest(http.MethodPost, srv.URL+"/remote", bytes.NewReader([]byte(`{"url":""}`)))
	req.Header.Set("Authorization", "token tok")
	r, _ := http.DefaultClient.Do(req)
	if r.StatusCode != http.StatusBadRequest {
		t.Errorf("expected 400, got %d", r.StatusCode)
	}
}

func getWith(t *testing.T, base, path string) *http.Response {
	t.Helper()
	req, _ := http.NewRequest(http.MethodGet, base+path, nil)
	req.Header.Set("Authorization", "token tok")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatalf("get: %v", err)
	}
	return resp
}

func TestCommitsBranchRefStillReturnsSHA(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()
	put(t, srv.URL, "notes/20250101-120000-abcd.json", `{"id":"x"}`, "")

	resp := getWith(t, srv.URL, "/repos/o/r/commits/draftnote")
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("status %d", resp.StatusCode)
	}
	var out map[string]string
	json.NewDecoder(resp.Body).Decode(&out)
	if out["sha"] == "" {
		t.Errorf("branch ref must still return {sha}, got %v", out)
	}
}

func TestCommitsHistoryReturnsList(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()
	path := "notes/20250101-120000-abcd.json"
	r1 := put(t, srv.URL, path, "a", "")
	var pr putResponse
	json.NewDecoder(r1.Body).Decode(&pr)
	put(t, srv.URL, path, "b", pr.Content.SHA)

	resp := getWith(t, srv.URL, "/repos/o/r/commits/draftnote?path="+path+"&per_page=30")
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("status %d", resp.StatusCode)
	}
	var list []commitListItem
	json.NewDecoder(resp.Body).Decode(&list)
	if len(list) != 2 {
		t.Fatalf("expected 2 commits, got %d", len(list))
	}
	if list[0].SHA == "" || list[0].Commit.Author.Date == "" {
		t.Errorf("commit item missing fields: %+v", list[0])
	}
}

func TestCommitDetailReturnsPatch(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()
	path := "notes/20250101-120000-abcd.json"
	r1 := put(t, srv.URL, path, "line1\n", "")
	var pr putResponse
	json.NewDecoder(r1.Body).Decode(&pr)
	put(t, srv.URL, path, "line1\nline2\n", pr.Content.SHA)

	hist := getWith(t, srv.URL, "/repos/o/r/commits/draftnote?path="+path)
	var list []commitListItem
	json.NewDecoder(hist.Body).Decode(&list)

	detail := getWith(t, srv.URL, "/repos/o/r/commits/"+list[0].SHA)
	if detail.StatusCode != http.StatusOK {
		t.Fatalf("detail status %d", detail.StatusCode)
	}
	var d commitDetailResponse
	json.NewDecoder(detail.Body).Decode(&d)
	found := false
	for _, f := range d.Files {
		if f.Filename == path && bytes.Contains([]byte(f.Patch), []byte("+line2")) {
			found = true
		}
	}
	if !found {
		t.Errorf("expected patch with +line2 for %s: %+v", path, d.Files)
	}
}

func TestContentsAtRefReturnsHistorical(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()
	path := "notes/20250101-120000-abcd.json"
	r1 := put(t, srv.URL, path, "old", "")
	var pr putResponse
	json.NewDecoder(r1.Body).Decode(&pr)
	put(t, srv.URL, path, "new", pr.Content.SHA)

	hist := getWith(t, srv.URL, "/repos/o/r/commits/draftnote?path="+path)
	var list []commitListItem
	json.NewDecoder(hist.Body).Decode(&list)
	oldSHA := list[1].SHA

	resp := getWith(t, srv.URL, "/repos/o/r/contents/"+path+"?ref="+oldSHA)
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("status %d", resp.StatusCode)
	}
	var f fileResponse
	json.NewDecoder(resp.Body).Decode(&f)
	decoded, _ := base64.StdEncoding.DecodeString(f.Content)
	if string(decoded) != "old" {
		t.Errorf("content at old ref = %q, want old", decoded)
	}
}

func TestCompareReturnsChanges(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()
	pathA := "notes/20250101-000000-aaaa.json"
	pathB := "notes/20250101-000000-bbbb.json"
	r1 := put(t, srv.URL, pathA, "A1", "")
	var pr putResponse
	json.NewDecoder(r1.Body).Decode(&pr)
	baseResp := getWith(t, srv.URL, "/repos/o/r/commits/draftnote")
	var baseHead map[string]string
	json.NewDecoder(baseResp.Body).Decode(&baseHead)
	base := baseHead["sha"]
	put(t, srv.URL, pathA, "A2", pr.Content.SHA)
	put(t, srv.URL, pathB, "B1", "")
	headResp := getWith(t, srv.URL, "/repos/o/r/commits/draftnote")
	var head map[string]string
	json.NewDecoder(headResp.Body).Decode(&head)
	resp := getWith(t, srv.URL, "/repos/o/r/compare/"+base+"..."+head["sha"])
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("status %d", resp.StatusCode)
	}
	var out compareResponse
	json.NewDecoder(resp.Body).Decode(&out)
	got := map[string]string{}
	for _, f := range out.Files {
		got[f.Filename] = f.Status
	}
	if got[pathA] != "modified" {
		t.Errorf("A status = %q", got[pathA])
	}
	if got[pathB] != "added" {
		t.Errorf("B status = %q", got[pathB])
	}
}

func TestCompareUnknownBaseErrors(t *testing.T) {
	srv := newServer(t)
	defer srv.Close()
	put(t, srv.URL, "notes/20250101-000000-aaaa.json", "A1", "")
	headResp := getWith(t, srv.URL, "/repos/o/r/commits/draftnote")
	var head map[string]string
	json.NewDecoder(headResp.Body).Decode(&head)
	resp := getWith(t, srv.URL, "/repos/o/r/compare/deadbeefdeadbeefdeadbeefdeadbeefdeadbeef..."+head["sha"])
	if resp.StatusCode == http.StatusOK {
		t.Errorf("compare with unknown base should not return 200")
	}
}
