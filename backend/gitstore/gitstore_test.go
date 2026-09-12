package gitstore

import (
	"fmt"
	"os"
	"os/exec"
	"strings"
	stdsync "sync"
	"testing"
)

func TestOpenSetsHEADToDraftnoteOnFreshRepo(t *testing.T) {
	dir := t.TempDir()
	_, err := Open(dir)
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	got := currentHEAD(t, dir)
	if got != "draftnote" {
		t.Errorf("HEAD = %q, want draftnote", got)
	}
}

func TestOpenReusesExistingDraftnoteHEAD(t *testing.T) {
	dir := t.TempDir()
	if _, err := Open(dir); err != nil {
		t.Fatalf("first open: %v", err)
	}

	s := &Store{dir: dir}
	if _, _, err := s.Write("notes/20250101-000000-abcd.json", "{}", "", "seed"); err != nil {
		t.Fatalf("write: %v", err)
	}
	if _, err := Open(dir); err != nil {
		t.Fatalf("re-open: %v", err)
	}
	got := currentHEAD(t, dir)
	if got != "draftnote" {
		t.Errorf("HEAD = %q, want draftnote", got)
	}
}

func currentHEAD(t *testing.T, dir string) string {
	t.Helper()
	cmd := exec.Command("git", "symbolic-ref", "--short", "HEAD")
	cmd.Dir = dir
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("symbolic-ref: %v", err)
	}
	return strings.TrimSpace(string(out))
}

func TestCloneOrOpenOpensExistingRepo(t *testing.T) {
	dir := t.TempDir()
	if _, err := CloneOrOpen(dir, ""); err != nil {
		t.Fatalf("first: %v", err)
	}
	s := &Store{dir: dir}
	if _, _, err := s.Write("notes/20250101-000000-abcd.json", "{}", "", "seed"); err != nil {
		t.Fatalf("write: %v", err)
	}
	if _, err := CloneOrOpen(dir, ""); err != nil {
		t.Fatalf("second: %v", err)
	}
	if got := currentHEAD(t, dir); got != "draftnote" {
		t.Errorf("HEAD = %q", got)
	}
}

func TestCloneOrOpenClonesFromLocalBareRepo(t *testing.T) {

	upstream := t.TempDir()
	run(t, upstream, "git", "init", "--bare")

	worktree := t.TempDir()
	run(t, worktree, "git", "init")
	run(t, worktree, "git", "config", "user.email", "test@local")
	run(t, worktree, "git", "config", "user.name", "test")
	run(t, worktree, "git", "symbolic-ref", "HEAD", "refs/heads/draftnote")
	seed := worktree + "/seed.txt"
	if err := writeFile(seed, "hello"); err != nil {
		t.Fatal(err)
	}
	run(t, worktree, "git", "add", "seed.txt")
	run(t, worktree, "git", "commit", "-m", "seed")
	run(t, worktree, "git", "remote", "add", "origin", upstream)
	run(t, worktree, "git", "push", "origin", "draftnote")

	target := t.TempDir() + "/draftnote"
	s, err := CloneOrOpen(target, upstream)
	if err != nil {
		t.Fatalf("clone: %v", err)
	}
	if got := currentHEAD(t, target); got != "draftnote" {
		t.Errorf("HEAD after clone = %q", got)
	}
	remote, err := s.RemoteURL()
	if err != nil {
		t.Fatalf("remote: %v", err)
	}
	if remote != upstream {
		t.Errorf("remote = %q, want %q", remote, upstream)
	}
	if !fileExists(target + "/seed.txt") {
		t.Errorf("cloned working tree missing seed.txt")
	}
}

func TestRemoteURLEmptyForInitOnly(t *testing.T) {
	dir := t.TempDir()
	s, err := CloneOrOpen(dir, "")
	if err != nil {
		t.Fatal(err)
	}
	url, err := s.RemoteURL()
	if err != nil {
		t.Fatal(err)
	}
	if url != "" {
		t.Errorf("expected empty remote for init'd repo, got %q", url)
	}
}

func TestRecloneReplacesContents(t *testing.T) {
	upstream := t.TempDir()
	run(t, upstream, "git", "init", "--bare")
	seed := t.TempDir()
	run(t, seed, "git", "init")
	run(t, seed, "git", "config", "user.email", "t@l")
	run(t, seed, "git", "config", "user.name", "t")
	run(t, seed, "git", "symbolic-ref", "HEAD", "refs/heads/draftnote")
	writeFile(seed+"/hello.txt", "world")
	run(t, seed, "git", "add", "hello.txt")
	run(t, seed, "git", "commit", "-m", "s")
	run(t, seed, "git", "remote", "add", "origin", upstream)
	run(t, seed, "git", "push", "origin", "draftnote")

	target := t.TempDir() + "/draftnote"
	s, err := CloneOrOpen(target, "")
	if err != nil {
		t.Fatal(err)
	}

	if err := s.Reclone(upstream); err != nil {
		t.Fatalf("reclone: %v", err)
	}
	url, _ := s.RemoteURL()
	if url != upstream {
		t.Errorf("origin after reclone = %q", url)
	}
	if !fileExists(target + "/hello.txt") {
		t.Errorf("reclone did not restore working tree")
	}
}

func run(t *testing.T, dir string, args ...string) {
	t.Helper()
	cmd := exec.Command(args[0], args[1:]...)
	cmd.Dir = dir
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("%v: %v: %s", args, err, out)
	}
}

func writeFile(path, body string) error {
	return os.WriteFile(path, []byte(body), 0o644)
}

func fileExists(path string) bool {
	_, err := os.Stat(path)
	return err == nil
}

func TestConcurrentWritesSerialize(t *testing.T) {
	store, err := Open(t.TempDir())
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	const n = 5
	var wg stdsync.WaitGroup
	errs := make([]error, n)
	for i := 0; i < n; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			path := fmt.Sprintf("notes/n%d.json", i)
			_, _, err := store.Write(path, fmt.Sprintf("body-%d", i), "", "seed "+path)
			errs[i] = err
		}(i)
	}
	wg.Wait()
	for i, e := range errs {
		if e != nil {
			t.Errorf("write %d: %v", i, e)
		}
	}
	entries, err := store.List("notes")
	if err != nil {
		t.Fatalf("list: %v", err)
	}
	if len(entries) != n {
		t.Errorf("expected %d entries, got %d", n, len(entries))
	}
	out, err := exec.Command("git", "-C", store.dir, "rev-list", "--count", "HEAD").CombinedOutput()
	if err != nil {
		t.Fatalf("rev-list: %v: %s", err, out)
	}
	got := strings.TrimSpace(string(out))
	if got != fmt.Sprintf("%d", n) {
		t.Errorf("expected %d commits, got %s", n, got)
	}
}

func TestFileHistoryPaginates(t *testing.T) {
	store, err := Open(t.TempDir())
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	path := "notes/20250101-000000-abcd.json"
	sha := ""
	for i := 0; i < 5; i++ {
		newSHA, _, err := store.Write(path, fmt.Sprintf(`{"body":"v%d"}`, i), sha, fmt.Sprintf("commit %d", i))
		if err != nil {
			t.Fatalf("write %d: %v", i, err)
		}
		sha = newSHA
	}
	page1, err := store.FileHistory(path, 0, 2)
	if err != nil {
		t.Fatalf("history page1: %v", err)
	}
	if len(page1) != 2 {
		t.Fatalf("page1 len = %d, want 2", len(page1))
	}
	if page1[0].Subject != "commit 4" {
		t.Errorf("newest subject = %q, want commit 4", page1[0].Subject)
	}
	page3, err := store.FileHistory(path, 4, 2)
	if err != nil {
		t.Fatalf("history page3: %v", err)
	}
	if len(page3) != 1 {
		t.Errorf("page3 len = %d, want 1", len(page3))
	}
}

func TestCommitFilesReturnsPatch(t *testing.T) {
	store, err := Open(t.TempDir())
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	path := "notes/20250101-000000-abcd.json"
	s1, _, _ := store.Write(path, "line1\n", "", "first")
	_, _, _ = store.Write(path, "line1\nline2\n", s1, "second")
	hist, err := store.FileHistory(path, 0, 10)
	if err != nil {
		t.Fatalf("history: %v", err)
	}
	files, err := store.CommitFiles(hist[0].SHA)
	if err != nil {
		t.Fatalf("commit files: %v", err)
	}
	found := false
	for _, f := range files {
		if f.Path == path {
			found = true
			if !strings.Contains(f.Patch, "+line2") {
				t.Errorf("patch missing +line2: %q", f.Patch)
			}
		}
	}
	if !found {
		t.Errorf("target path not in commit files: %+v", files)
	}
}

func TestReadAtRefReturnsHistoricalContent(t *testing.T) {
	store, err := Open(t.TempDir())
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	path := "notes/20250101-000000-abcd.json"
	s1, _, _ := store.Write(path, "old content", "", "first")
	_, _, _ = store.Write(path, "new content", s1, "second")
	hist, _ := store.FileHistory(path, 0, 10)
	oldSHA := hist[1].SHA
	content, blob, err := store.ReadAtRef(path, oldSHA)
	if err != nil {
		t.Fatalf("read at ref: %v", err)
	}
	if content != "old content" {
		t.Errorf("content = %q, want old content", content)
	}
	if blob == "" {
		t.Error("expected blob sha")
	}
}

func TestReadAtRefRejectsBadRef(t *testing.T) {
	store, err := Open(t.TempDir())
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	if _, _, err := store.ReadAtRef("notes/a.json", "; rm -rf /"); err != ErrInvalidPath {
		t.Errorf("expected ErrInvalidPath for hostile ref, got %v", err)
	}
}

func TestCompareReportsChanges(t *testing.T) {
	store, err := Open(t.TempDir())
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	pathA := "notes/20250101-000000-aaaa.json"
	pathB := "notes/20250101-000000-bbbb.json"
	pathC := "notes/20250101-000000-cccc.json"
	sA, _, _ := store.Write(pathA, "A1\n", "", "add A")
	_, _, _ = store.Write(pathB, "B1\n", "", "add B")
	base, err := store.HeadSHA()
	if err != nil {
		t.Fatalf("head: %v", err)
	}
	_, _, _ = store.Write(pathA, "A2\n", sA, "mod A")
	_, _ = store.Delete(pathB, "", "del B")
	_, _, _ = store.Write(pathC, "C1\n", "", "add C")
	head, err := store.HeadSHA()
	if err != nil {
		t.Fatalf("head: %v", err)
	}
	changes, err := store.Compare(base, head)
	if err != nil {
		t.Fatalf("compare: %v", err)
	}
	got := map[string]string{}
	for _, c := range changes {
		got[c.Path] = c.Status
	}
	if got[pathA] != "modified" {
		t.Errorf("A status = %q, want modified", got[pathA])
	}
	if got[pathB] != "removed" {
		t.Errorf("B status = %q, want removed", got[pathB])
	}
	if got[pathC] != "added" {
		t.Errorf("C status = %q, want added", got[pathC])
	}
}

func TestCompareRejectsBadRef(t *testing.T) {
	store, err := Open(t.TempDir())
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	if _, err := store.Compare("draftnote", "; rm -rf /"); err != ErrInvalidPath {
		t.Errorf("expected ErrInvalidPath, got %v", err)
	}
}
