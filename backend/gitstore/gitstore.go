package gitstore

import (
	"bytes"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	stdsync "sync"
)

var (
	ErrNotFound    = errors.New("not found")
	ErrInvalidPath = errors.New("invalid path")
)

var safeRelPathPattern = regexp.MustCompile(`^[A-Za-z0-9_./][A-Za-z0-9_./-]*$`)

var safeRefPattern = regexp.MustCompile(`^([0-9a-f]{7,40}|draftnote)$`)

func validateRef(ref string) error {
	if !safeRefPattern.MatchString(ref) {
		return ErrInvalidPath
	}
	return nil
}

func validateRelPath(p string) error {
	if p == "" || len(p) > 512 {
		return ErrInvalidPath
	}
	if strings.ContainsRune(p, 0x00) {
		return ErrInvalidPath
	}
	if !safeRelPathPattern.MatchString(p) {
		return ErrInvalidPath
	}
	if strings.Contains(p, "..") {
		return ErrInvalidPath
	}
	return nil
}

type Store struct {
	dir string
	mu  stdsync.Mutex
}

func Open(dir string) (*Store, error) {
	return CloneOrOpen(dir, "")
}

func CloneOrOpen(dir, remoteURL string) (*Store, error) {
	abs, err := filepath.Abs(dir)
	if err != nil {
		return nil, err
	}
	s := &Store{dir: abs}
	if _, err := os.Stat(filepath.Join(abs, ".git")); err == nil {
		if err := s.ensureDraftnoteHEAD(); err != nil {
			return nil, err
		}
		return s, nil
	}
	if remoteURL != "" {
		if err := os.MkdirAll(filepath.Dir(abs), 0o755); err != nil {
			return nil, err
		}
		cmd := exec.Command("git", "clone", "--", remoteURL, abs)
		if out, err := cmd.CombinedOutput(); err != nil {
			return nil, fmt.Errorf("clone %s: %v: %s", remoteURL, err, strings.TrimSpace(string(out)))
		}
		if err := s.gitSetConfig("user.email", "draftnote@local"); err != nil {
			return nil, err
		}
		if err := s.gitSetConfig("user.name", "draftnote"); err != nil {
			return nil, err
		}
		return s, s.checkoutDraftnote()
	}
	if err := os.MkdirAll(abs, 0o755); err != nil {
		return nil, err
	}
	if err := s.gitInitRepo(); err != nil {
		return nil, err
	}
	if err := s.gitSetConfig("user.email", "draftnote@local"); err != nil {
		return nil, err
	}
	if err := s.gitSetConfig("user.name", "draftnote"); err != nil {
		return nil, err
	}
	if err := s.ensureDraftnoteHEAD(); err != nil {
		return nil, err
	}
	return s, nil
}

func (s *Store) checkoutDraftnote() error {
	if _, err := s.runCmd(exec.Command("git", "rev-parse", "--verify", "refs/heads/draftnote")); err == nil {
		_, err := s.runCmd(exec.Command("git", "checkout", "draftnote"))
		return err
	}
	if _, err := s.runCmd(exec.Command("git", "rev-parse", "--verify", "refs/remotes/origin/draftnote")); err == nil {
		_, err := s.runCmd(exec.Command("git", "checkout", "-B", "draftnote", "origin/draftnote"))
		return err
	}
	return s.ensureDraftnoteHEAD()
}

func (s *Store) RemoteURL() (string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	out, err := s.runCmd(exec.Command("git", "remote", "get-url", "origin"))
	if err != nil {
		return "", nil
	}
	return strings.TrimSpace(out), nil
}

func (s *Store) Reclone(newURL string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	newURL = strings.TrimSpace(newURL)
	if newURL == "" {
		return errors.New("empty remote URL")
	}
	entries, err := os.ReadDir(s.dir)
	if err != nil {
		return err
	}
	for _, e := range entries {
		if err := os.RemoveAll(filepath.Join(s.dir, e.Name())); err != nil {
			return err
		}
	}
	cmd := exec.Command("git", "clone", "--", newURL, s.dir)
	if out, err := cmd.CombinedOutput(); err != nil {
		return fmt.Errorf("clone %s: %v: %s", newURL, err, strings.TrimSpace(string(out)))
	}
	if err := s.gitSetConfig("user.email", "draftnote@local"); err != nil {
		return err
	}
	if err := s.gitSetConfig("user.name", "draftnote"); err != nil {
		return err
	}
	return s.checkoutDraftnote()
}

func (s *Store) ensureDraftnoteHEAD() error {
	_, err := s.runCmd(exec.Command("git", "symbolic-ref", "HEAD", "refs/heads/draftnote"))
	return err
}

func (s *Store) safeFullPath(p string) (string, error) {
	if p == "" || strings.ContainsRune(p, 0x00) {
		return "", ErrInvalidPath
	}
	if filepath.IsAbs(p) {
		return "", ErrInvalidPath
	}
	cleaned := filepath.Clean("/" + filepath.ToSlash(p))
	if cleaned == "/" {
		return "", ErrInvalidPath
	}
	full := filepath.Join(s.dir, cleaned)
	rel, err := filepath.Rel(s.dir, full)
	if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
		return "", ErrInvalidPath
	}
	return full, nil
}

func (s *Store) relPath(p string) (string, error) {
	full, err := s.safeFullPath(p)
	if err != nil {
		return "", err
	}
	return filepath.Rel(s.dir, full)
}

func (s *Store) runCmd(cmd *exec.Cmd) (string, error) {
	cmd.Dir = s.dir
	var out, errb bytes.Buffer
	cmd.Stdout = &out
	cmd.Stderr = &errb
	if err := cmd.Run(); err != nil {
		return "", errors.New(strings.TrimSpace(errb.String()))
	}
	return out.String(), nil
}

func (s *Store) gitInitRepo() error {
	_, err := s.runCmd(exec.Command("git", "init"))
	return err
}

func (s *Store) gitSetConfig(key, value string) error {

	switch key {
	case "user.email", "user.name":
	default:
		return fmt.Errorf("config key not allowed: %q", key)
	}
	if strings.ContainsRune(value, 0x00) || strings.HasPrefix(value, "-") {
		return ErrInvalidPath
	}
	_, err := s.runCmd(exec.Command("git", "config", key, value))
	return err
}

func (s *Store) gitHashObjectByRel(relPath string) (string, error) {
	if err := validateRelPath(relPath); err != nil {
		return "", err
	}
	out, err := s.runCmd(exec.Command("git", "hash-object", "--", relPath))
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(out), nil
}

func (s *Store) gitAddByRel(relPath string) error {
	if err := validateRelPath(relPath); err != nil {
		return err
	}
	_, err := s.runCmd(exec.Command("git", "add", "--", relPath))
	return err
}

func (s *Store) gitRmByRel(relPath string) error {
	if err := validateRelPath(relPath); err != nil {
		return err
	}
	_, err := s.runCmd(exec.Command("git", "rm", "-f", "--", relPath))
	return err
}

func (s *Store) gitCommit(message string) error {

	cleaned := strings.ReplaceAll(message, "\x00", "")
	cleaned = strings.ReplaceAll(cleaned, "\n", " ")
	_, err := s.runCmd(exec.Command("git", "commit", "-m", cleaned))
	return err
}

func (s *Store) HeadSHA() (string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	out, err := s.runCmd(exec.Command("git", "rev-parse", "--quiet", "--verify", "HEAD"))
	if err != nil {
		return "", nil
	}
	return strings.TrimSpace(out), nil
}

func (s *Store) BlobSHA(path string) (string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.blobSHA(path)
}

func (s *Store) blobSHA(path string) (string, error) {
	rel, err := s.relPath(path)
	if err != nil {
		return "", err
	}
	rel = filepath.ToSlash(rel)
	return s.gitHashObjectByRel(rel)
}

func (s *Store) Exists(path string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.exists(path)
}

func (s *Store) exists(path string) bool {
	full, err := s.safeFullPath(path)
	if err != nil {
		return false
	}
	_, err = os.Stat(full)
	return err == nil
}

func (s *Store) IsDir(path string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	full, err := s.safeFullPath(path)
	if err != nil {
		return false
	}
	fi, err := os.Stat(full)
	return err == nil && fi.IsDir()
}

type Entry struct {
	Name string
	Path string
	SHA  string
}

func (s *Store) List(dir string) ([]Entry, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	full, err := s.safeFullPath(dir)
	if err != nil {
		return nil, err
	}
	items, err := os.ReadDir(full)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, err
	}
	cleaned := filepath.ToSlash(filepath.Clean(dir))
	var entries []Entry
	for _, it := range items {
		if it.IsDir() {
			continue
		}
		rel := cleaned + "/" + it.Name()
		sha, err := s.blobSHA(rel)
		if err != nil {
			return nil, err
		}
		entries = append(entries, Entry{Name: it.Name(), Path: rel, SHA: sha})
	}
	return entries, nil
}

func (s *Store) Read(path string) (content string, sha string, err error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	full, err := s.safeFullPath(path)
	if err != nil {
		return "", "", err
	}
	b, err := os.ReadFile(full)
	if err != nil {
		if os.IsNotExist(err) {
			return "", "", ErrNotFound
		}
		return "", "", err
	}
	sha, err = s.blobSHA(path)
	if err != nil {
		return "", "", err
	}
	return string(b), sha, nil
}

func (s *Store) Write(path, content, expectSHA, message string) (newSHA string, conflict bool, err error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	full, err := s.safeFullPath(path)
	if err != nil {
		return "", false, err
	}
	rel, err := s.relPath(path)
	if err != nil {
		return "", false, err
	}
	rel = filepath.ToSlash(rel)
	if err := validateRelPath(rel); err != nil {
		return "", false, err
	}
	if s.exists(path) {
		current, err := s.blobSHA(path)
		if err != nil {
			return "", false, err
		}
		if expectSHA != current {
			return "", true, nil
		}
	} else if expectSHA != "" {
		return "", true, nil
	}
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		return "", false, err
	}
	if err := os.WriteFile(full, []byte(content), 0o644); err != nil {
		return "", false, err
	}
	if err := s.gitAddByRel(rel); err != nil {
		return "", false, err
	}
	if err := s.gitCommit(message); err != nil {
		return "", false, err
	}
	newSHA, err = s.blobSHA(path)
	if err != nil {
		return "", false, err
	}
	return newSHA, false, nil
}

func (s *Store) Delete(path, expectSHA, message string) (conflict bool, err error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if !s.exists(path) {
		return false, ErrNotFound
	}
	rel, err := s.relPath(path)
	if err != nil {
		return false, err
	}
	rel = filepath.ToSlash(rel)
	if err := validateRelPath(rel); err != nil {
		return false, err
	}
	if expectSHA != "" {
		current, err := s.blobSHA(path)
		if err != nil {
			return false, err
		}
		if current != expectSHA {
			return true, nil
		}
	}
	if err := s.gitRmByRel(rel); err != nil {
		return false, err
	}
	return false, s.gitCommit(message)
}

type CommitMeta struct {
	SHA        string
	AuthorName string
	Date       string
	Subject    string
}

type FilePatch struct {
	Path  string
	Patch string
}

func (s *Store) FileHistory(path string, skip, max int) ([]CommitMeta, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	rel, err := s.relPath(path)
	if err != nil {
		return nil, err
	}
	rel = filepath.ToSlash(rel)
	if err := validateRelPath(rel); err != nil {
		return nil, err
	}
	if skip < 0 {
		skip = 0
	}
	if max <= 0 {
		max = 30
	}
	args := []string{
		"log",
		fmt.Sprintf("--skip=%d", skip),
		fmt.Sprintf("--max-count=%d", max),
		"--format=%H%x00%an%x00%aI%x00%s",
		"--", rel,
	}
	out, err := s.runCmd(exec.Command("git", args...))
	if err != nil {
		return nil, err
	}
	var metas []CommitMeta
	for _, line := range strings.Split(out, "\n") {
		if strings.TrimSpace(line) == "" {
			continue
		}
		parts := strings.SplitN(line, "\x00", 4)
		if len(parts) != 4 {
			continue
		}
		metas = append(metas, CommitMeta{
			SHA:        parts[0],
			AuthorName: parts[1],
			Date:       parts[2],
			Subject:    parts[3],
		})
	}
	return metas, nil
}

func (s *Store) CommitFiles(sha string) ([]FilePatch, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := validateRef(sha); err != nil {
		return nil, err
	}
	out, err := s.runCmd(exec.Command("git", "show", "--format=", "--patch", sha))
	if err != nil {
		return nil, err
	}
	return parseUnifiedDiff(out), nil
}

type FileChange struct {
	Path   string
	Status string
}

func (s *Store) Compare(base, head string) ([]FileChange, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := validateRef(base); err != nil {
		return nil, err
	}
	if err := validateRef(head); err != nil {
		return nil, err
	}
	out, err := s.runCmd(exec.Command("git", "diff", "--name-status", "-z", base+"..."+head))
	if err != nil {
		return nil, err
	}
	tokens := strings.Split(out, "\x00")
	var changes []FileChange
	for i := 0; i < len(tokens); {
		status := strings.TrimSpace(tokens[i])
		if status == "" {
			i++
			continue
		}
		i++
		if i >= len(tokens) {
			break
		}
		path := tokens[i]
		i++
		mapped := status
		switch {
		case strings.HasPrefix(status, "A"):
			mapped = "added"
		case strings.HasPrefix(status, "M"):
			mapped = "modified"
		case strings.HasPrefix(status, "D"):
			mapped = "removed"
		case strings.HasPrefix(status, "R") || strings.HasPrefix(status, "C"):
			if i < len(tokens) {
				newPath := tokens[i]
				i++
				changes = append(changes, FileChange{Path: path, Status: "removed"})
				changes = append(changes, FileChange{Path: newPath, Status: "added"})
			}
			continue
		case strings.HasPrefix(status, "T"):
			mapped = "modified"
		}
		changes = append(changes, FileChange{Path: path, Status: mapped})
	}
	return changes, nil
}

func (s *Store) ReadAtRef(path, ref string) (content string, sha string, err error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := validateRef(ref); err != nil {
		return "", "", err
	}
	rel, err := s.relPath(path)
	if err != nil {
		return "", "", err
	}
	rel = filepath.ToSlash(rel)
	if err := validateRelPath(rel); err != nil {
		return "", "", err
	}
	spec := ref + ":" + rel
	out, err := s.runCmd(exec.Command("git", "show", spec))
	if err != nil {
		return "", "", ErrNotFound
	}
	shaOut, err := s.runCmd(exec.Command("git", "rev-parse", spec))
	if err != nil {
		return "", "", err
	}
	return out, strings.TrimSpace(shaOut), nil
}

func parseUnifiedDiff(out string) []FilePatch {
	var patches []FilePatch
	lines := strings.Split(out, "\n")
	var cur *FilePatch
	var buf []string
	flush := func() {
		if cur != nil {
			cur.Patch = strings.Join(buf, "\n")
			patches = append(patches, *cur)
		}
		cur = nil
		buf = nil
	}
	for _, line := range lines {
		if strings.HasPrefix(line, "diff --git ") {
			flush()
			path := extractDiffPath(line)
			cur = &FilePatch{Path: path}
			buf = []string{line}
			continue
		}
		if cur != nil {
			buf = append(buf, line)
		}
	}
	flush()
	return patches
}

func extractDiffPath(line string) string {
	fields := strings.Fields(line)
	if len(fields) < 4 {
		return ""
	}
	b := fields[3]
	return strings.TrimPrefix(b, "b/")
}
