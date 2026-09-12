package local

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

func testStore(t *testing.T) *Store {
	t.Helper()
	s, err := Open(t.TempDir())
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	tick := time.Date(2026, 9, 1, 14, 30, 22, 0, time.UTC)
	s.now = func() time.Time {
		tick = tick.Add(time.Second)
		return tick
	}
	return s
}

func TestCreateLoadRoundTrip(t *testing.T) {
	s := testStore(t)
	n, err := s.Create("标题", []string{"医案"}, "# 正文\n内容")
	if err != nil {
		t.Fatalf("create: %v", err)
	}
	if n.ID == "" {
		t.Fatal("empty id")
	}
	got, err := s.Load(n.ID)
	if err != nil {
		t.Fatalf("load: %v", err)
	}
	if got.Body != "# 正文\n内容" || got.Title != "标题" || !got.HasTag("医案") {
		t.Errorf("mismatch: %+v", got)
	}
}

func TestNewIDStableFormatAndUnique(t *testing.T) {
	s := testStore(t)
	id1, _ := s.NewID()
	id2, _ := s.NewID()
	if id1 == id2 {
		t.Fatal("not unique")
	}
	if len(id1) != len("20260901-143023-abcd") {
		t.Errorf("id format: %q", id1)
	}
}

func TestTitleChangeKeepsFilename(t *testing.T) {
	s := testStore(t)
	n, _ := s.Create("旧标题", nil, "body")
	id := n.ID
	newTitle := "新标题"
	if _, err := s.Update(id, &newTitle, nil, "body2"); err != nil {
		t.Fatalf("update: %v", err)
	}
	if _, err := os.Stat(s.notePath(id)); err != nil {
		t.Errorf("file renamed after title change: %v", err)
	}
	got, _ := s.Load(id)
	if got.Title != "新标题" || got.Body != "body2" {
		t.Errorf("update not applied: %+v", got)
	}
}

func TestUpdateBumpsUpdatedNotCreated(t *testing.T) {
	s := testStore(t)
	n, _ := s.Create("t", nil, "a")
	created := n.Created
	got, _ := s.Update(n.ID, nil, nil, "b")
	if !got.Created.Equal(created) {
		t.Error("Created must not change")
	}
	if !got.Updated.After(created) {
		t.Error("Updated must advance")
	}
}

func TestDeleteIdempotentAndClearsSHA(t *testing.T) {
	s := testStore(t)
	n, _ := s.Create("t", nil, "a")
	if err := s.SetSHA(n.ID, "abc"); err != nil {
		t.Fatalf("setsha: %v", err)
	}
	if err := s.Delete(n.ID); err != nil {
		t.Fatalf("delete: %v", err)
	}
	if _, ok, _ := s.SHA(n.ID); ok {
		t.Error("sha not cleared on delete")
	}
	if err := s.Delete(n.ID); err != nil {
		t.Errorf("second delete: %v", err)
	}
	if _, err := s.Load(n.ID); err == nil {
		t.Error("load after delete should fail")
	}
}

func TestListSortedByUpdatedDesc(t *testing.T) {
	s := testStore(t)
	a, _ := s.Create("A", nil, "a")
	b, _ := s.Create("B", nil, "b")
	list, err := s.List()
	if err != nil {
		t.Fatalf("list: %v", err)
	}
	if len(list) != 2 || list[0].ID != b.ID || list[1].ID != a.ID {
		t.Errorf("expected newest first: %v", list)
	}
}

func TestPurgeExpiredTrash(t *testing.T) {
	s := testStore(t)
	n1, _ := s.Create("keep", nil, "a")
	n2, _ := s.Create("expire", nil, "b")
	if err := s.SetTrashed(n2.ID, true); err != nil {
		t.Fatalf("trash: %v", err)
	}
	tick := s.now().Add(TrashRetention + 24*time.Hour)
	s.now = func() time.Time { return tick }
	purged, err := s.PurgeExpiredTrash()
	if err != nil {
		t.Fatalf("purge: %v", err)
	}
	if len(purged) != 1 || purged[0] != n2.ID {
		t.Errorf("purged = %v want %s", purged, n2.ID)
	}
	if _, err := s.Load(n2.ID); err == nil {
		t.Error("expired note should be gone")
	}
	if _, err := s.Load(n1.ID); err != nil {
		t.Errorf("kept note missing: %v", err)
	}
}

func TestPurgeSkipsRecentTrash(t *testing.T) {
	s := testStore(t)
	n, _ := s.Create("recent", nil, "x")
	s.SetTrashed(n.ID, true)
	purged, err := s.PurgeExpiredTrash()
	if err != nil {
		t.Fatalf("purge: %v", err)
	}
	if len(purged) != 0 {
		t.Errorf("recent trash should not be purged: %v", purged)
	}
	if _, err := s.Load(n.ID); err != nil {
		t.Errorf("recent trash should still exist: %v", err)
	}
}

func TestSetTrashedRecordsTimestamp(t *testing.T) {
	s := testStore(t)
	n, _ := s.Create("t", nil, "x")
	if err := s.SetTrashed(n.ID, true); err != nil {
		t.Fatalf("trash: %v", err)
	}
	got, _ := s.Load(n.ID)
	if !got.Trashed || got.TrashedAt == nil {
		t.Errorf("expected trashed with timestamp, got %+v", got)
	}
	if err := s.SetTrashed(n.ID, false); err != nil {
		t.Fatalf("untrash: %v", err)
	}
	got, _ = s.Load(n.ID)
	if got.Trashed || got.TrashedAt != nil {
		t.Errorf("expected clean, got trashed=%v ts=%v", got.Trashed, got.TrashedAt)
	}
}

func TestRejectsPathTraversalID(t *testing.T) {
	s := testStore(t)
	outside := filepath.Join(s.root, "outside.json")
	if err := os.WriteFile(outside, []byte("{}"), 0o644); err != nil {
		t.Fatalf("prep outside: %v", err)
	}
	legit, err := s.Create("t", nil, "body")
	if err != nil {
		t.Fatalf("create legit: %v", err)
	}

	bad := []string{
		"../outside",
		"..",
		".",
		"",
		"a/b",
		"a\\b",
		"a\x00b",
		"a b",
		"-flag",
		"a.json",
		"日本語",
		"20260901-143022-a3f2/../evil",
	}
	for _, id := range bad {
		t.Run("Load/"+id, func(t *testing.T) {
			if _, err := s.Load(id); err == nil {
				t.Errorf("Load(%q) should reject invalid id", id)
			}
		})
		t.Run("Delete/"+id, func(t *testing.T) {
			if err := s.Delete(id); err == nil {
				t.Errorf("Delete(%q) should reject invalid id", id)
			}
		})
		t.Run("Update/"+id, func(t *testing.T) {
			if _, err := s.Update(id, nil, nil, "x"); err == nil {
				t.Errorf("Update(%q) should reject invalid id", id)
			}
		})
		t.Run("SetPinned/"+id, func(t *testing.T) {
			if err := s.SetPinned(id, true); err == nil {
				t.Errorf("SetPinned(%q) should reject invalid id", id)
			}
		})
		t.Run("SetTrashed/"+id, func(t *testing.T) {
			if err := s.SetTrashed(id, true); err == nil {
				t.Errorf("SetTrashed(%q) should reject invalid id", id)
			}
		})
		t.Run("Duplicate/"+id, func(t *testing.T) {
			if _, err := s.Duplicate(id); err == nil {
				t.Errorf("Duplicate(%q) should reject invalid id", id)
			}
		})
	}

	if _, err := os.Stat(outside); err != nil {
		t.Errorf("outside file was affected by traversal attempt: %v", err)
	}
	if _, err := s.Load(legit.ID); err != nil {
		t.Errorf("legit id broke after guard: %v", err)
	}
}

func TestSHARoundTrip(t *testing.T) {
	s := testStore(t)
	if _, ok, _ := s.SHA("missing"); ok {
		t.Error("missing sha should be absent")
	}
	if err := s.SetSHA("id1", "sha1"); err != nil {
		t.Fatalf("setsha: %v", err)
	}
	sha, ok, err := s.SHA("id1")
	if err != nil || !ok || sha != "sha1" {
		t.Errorf("SHA = %q %v %v", sha, ok, err)
	}
	if _, err := os.Stat(filepath.Join(s.root, stateFile)); err != nil {
		t.Errorf("sync-state.json not written: %v", err)
	}
}
