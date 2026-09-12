package app

import (
	"testing"
)

func testApp(t *testing.T) *App {
	t.Helper()
	a, err := New(Config{
		Root:     t.TempDir(),
		BaseURL:  "http://example.invalid",
		Owner:    "o",
		Repo:     "r",
		Username: "",
		Token:    "tok",
		UseAPI:   true,
	})
	if err != nil {
		t.Fatalf("new: %v", err)
	}
	return a
}

func TestCreateListLoad(t *testing.T) {
	a := testApp(t)
	n, err := a.CreateNote("标题", []string{"医案"}, "body")
	if err != nil {
		t.Fatalf("create: %v", err)
	}
	list, err := a.ListNotes()
	if err != nil || len(list) != 1 {
		t.Fatalf("list: %v %d", err, len(list))
	}
	got, err := a.LoadNote(n.ID)
	if err != nil || got.Body != "body" {
		t.Errorf("load: %v %q", err, got.Body)
	}
}

func TestTagsAndFilter(t *testing.T) {
	a := testApp(t)
	a.CreateNote("A", []string{"医案", "太阳病"}, "a")
	a.CreateNote("B", []string{"笔记"}, "b")

	tags, err := a.AllTags()
	if err != nil || len(tags) != 3 {
		t.Fatalf("tags: %v %v", tags, err)
	}
	byTag, err := a.NotesByTag("医案")
	if err != nil || len(byTag) != 1 {
		t.Errorf("byTag: %v %d", err, len(byTag))
	}
}

func TestUpdateAndDelete(t *testing.T) {
	a := testApp(t)
	n, _ := a.CreateNote("t", nil, "a")
	up, err := a.UpdateNote(n.ID, "t2", []string{"x"}, "b")
	if err != nil || up.Title != "t2" || up.Body != "b" {
		t.Errorf("update: %v %+v", err, up)
	}
	if err := a.DeleteNote(n.ID); err != nil {
		t.Fatalf("delete: %v", err)
	}
	if _, err := a.LoadNote(n.ID); err == nil {
		t.Error("load after delete should fail")
	}
}

func TestListNotesMarksSyncFailed(t *testing.T) {
	a := testApp(t)
	n, err := a.CreateNote("t", nil, "body")
	if err != nil {
		t.Fatalf("create: %v", err)
	}
	if _, err := a.PushNote(n.ID); err == nil {
		t.Fatal("expected push to fail against invalid host")
	}
	metas, err := a.ListNotes()
	if err != nil {
		t.Fatalf("list: %v", err)
	}
	found := false
	for _, m := range metas {
		if m.ID == n.ID {
			found = true
			if !m.SyncFailed {
				t.Errorf("expected SyncFailed=true for %s", n.ID)
			}
		}
	}
	if !found {
		t.Errorf("note %s not in list", n.ID)
	}
}

func TestSyncFailedHiddenWhenUnconfigured(t *testing.T) {
	a, err := New(Config{Root: t.TempDir()})
	if err != nil {
		t.Fatalf("new: %v", err)
	}
	n, err := a.CreateNote("t", nil, "body")
	if err != nil {
		t.Fatalf("create: %v", err)
	}
	a.PushNote(n.ID)
	metas, err := a.ListNotes()
	if err != nil {
		t.Fatalf("list: %v", err)
	}
	for _, m := range metas {
		if m.ID == n.ID && m.SyncFailed {
			t.Errorf("SyncFailed should be hidden when sync is not configured")
		}
	}
}
