package server

import (
	"net/http/httptest"
	"strings"
	"testing"

	"draftnote/backend/gitstore"
	"draftnote/internal/app"
	"draftnote/internal/syncclient"
)

func TestBackendServesSyncclientBasicAuth(t *testing.T) {
	store, err := gitstore.Open(t.TempDir())
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	backend := &Server{Store: store, Username: "alice", Token: "s3cret"}
	srv := httptest.NewServer(backend.Handler())
	defer srv.Close()

	client := syncclient.New(srv.URL, "o", "r", "alice", "s3cret", false)

	newSHA, err := client.Put("notes/20260901-120000-abcd.json", `{"id":"20260901-120000-abcd","body":"hi"}`, "", "seed")
	if err != nil {
		t.Fatalf("put: %v", err)
	}
	if newSHA == "" {
		t.Fatal("expected sha from put")
	}

	content, sha, err := client.Get("notes/20260901-120000-abcd.json")
	if err != nil {
		t.Fatalf("get: %v", err)
	}
	if sha != newSHA {
		t.Errorf("sha mismatch: put %q, get %q", newSHA, sha)
	}
	if !strings.Contains(content, `"body":"hi"`) {
		t.Errorf("unexpected body: %q", content)
	}
}

func TestBackendServesSyncclientTokenAuth(t *testing.T) {
	store, err := gitstore.Open(t.TempDir())
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	backend := &Server{Store: store, Username: "alice", Token: "s3cret"}
	srv := httptest.NewServer(backend.Handler())
	defer srv.Close()

	client := syncclient.New(srv.URL, "o", "r", "", "s3cret", true)

	if _, err := client.Put("notes/20260901-130000-beef.json", `{"id":"20260901-130000-beef"}`, "", "seed"); err != nil {
		t.Fatalf("put: %v", err)
	}
	entries, err := client.List("notes")
	if err != nil {
		t.Fatalf("list: %v", err)
	}
	if len(entries) != 1 || entries[0].Name != "20260901-130000-beef.json" {
		t.Fatalf("unexpected entries: %+v", entries)
	}
}

func TestSyncclientRevisionRoundTrip(t *testing.T) {
	store, err := gitstore.Open(t.TempDir())
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	backend := &Server{Store: store, Username: "alice", Token: "s3cret"}
	srv := httptest.NewServer(backend.Handler())
	defer srv.Close()

	client := syncclient.New(srv.URL, "o", "r", "", "s3cret", true)
	path := "notes/20260901-120000-abcd.json"

	s1, err := client.Put(path, "old body", "", "first")
	if err != nil {
		t.Fatalf("put1: %v", err)
	}
	if _, err := client.Put(path, "new body", s1, "second"); err != nil {
		t.Fatalf("put2: %v", err)
	}

	hist, err := client.FileHistory(path, 1, 30)
	if err != nil {
		t.Fatalf("history: %v", err)
	}
	if len(hist) != 2 {
		t.Fatalf("expected 2 revisions, got %d", len(hist))
	}
	if hist[0].Subject != "second" {
		t.Errorf("newest subject = %q, want second", hist[0].Subject)
	}

	patch, err := client.CommitPatch(hist[0].SHA, path)
	if err != nil {
		t.Fatalf("commit patch: %v", err)
	}
	if !strings.Contains(patch, "new body") {
		t.Errorf("patch missing new body: %q", patch)
	}

	content, _, err := client.GetAtRef(path, hist[1].SHA)
	if err != nil {
		t.Fatalf("get at ref: %v", err)
	}
	if content != "old body" {
		t.Errorf("historical content = %q, want old body", content)
	}
}

func TestAppRestoreRevisionForwardRollback(t *testing.T) {
	store, err := gitstore.Open(t.TempDir())
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	backend := &Server{Store: store, Token: "s3cret"}
	srv := httptest.NewServer(backend.Handler())
	defer srv.Close()

	a, err := app.New(app.Config{
		Root:    t.TempDir(),
		BaseURL: srv.URL,
		Owner:   "o",
		Repo:    "r",
		Token:   "s3cret",
		UseAPI:  true,
	})
	if err != nil {
		t.Fatalf("app new: %v", err)
	}

	n, err := a.CreateNote("t", nil, "version one")
	if err != nil {
		t.Fatalf("create: %v", err)
	}
	if _, err := a.PushNote(n.ID); err != nil {
		t.Fatalf("push v1: %v", err)
	}
	if _, err := a.UpdateNote(n.ID, "t", nil, "version two"); err != nil {
		t.Fatalf("update: %v", err)
	}
	if _, err := a.PushNote(n.ID); err != nil {
		t.Fatalf("push v2: %v", err)
	}

	revs, err := a.RevisionList(n.ID, 1, 30)
	if err != nil {
		t.Fatalf("revision list: %v", err)
	}
	if len(revs) != 2 {
		t.Fatalf("expected 2 revisions, got %d", len(revs))
	}

	oldSHA := revs[1].SHA
	if err := a.RestoreRevision(n.ID, oldSHA); err != nil {
		t.Fatalf("restore: %v", err)
	}

	loaded, err := a.LoadNote(n.ID)
	if err != nil {
		t.Fatalf("load: %v", err)
	}
	if loaded.Body != "version one" {
		t.Errorf("body after restore = %q, want version one", loaded.Body)
	}

	after, err := a.RevisionList(n.ID, 1, 30)
	if err != nil {
		t.Fatalf("revision list after: %v", err)
	}
	if len(after) != 3 {
		t.Errorf("expected 3 revisions after forward rollback, got %d", len(after))
	}
}
