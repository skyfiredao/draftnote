package local

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"

	"draftnote/internal/note"
)

const (
	notesDir  = "notes"
	stateFile = "sync-state.json"
)

var ErrInvalidID = errors.New("invalid note id")

var stableIDPattern = regexp.MustCompile(`^[0-9]{8}-[0-9]{6}-[0-9a-f]{4}$`)

func validateID(id string) error {
	if !stableIDPattern.MatchString(id) {
		return ErrInvalidID
	}
	return nil
}

func ValidateID(id string) error { return validateID(id) }

type Store struct {
	root string
	now  func() time.Time
}

func Open(root string) (*Store, error) {
	s := &Store{root: root, now: time.Now}
	if err := os.MkdirAll(filepath.Join(root, notesDir), 0o755); err != nil {
		return nil, err
	}
	return s, nil
}

func (s *Store) notePath(id string) string {
	return filepath.Join(s.root, notesDir, id+".json")
}

func (s *Store) NewID() (string, error) {
	var b [2]byte
	if _, err := rand.Read(b[:]); err != nil {
		return "", err
	}
	return s.now().UTC().Format("20060102-150405") + "-" + hex.EncodeToString(b[:]), nil
}

func (s *Store) Create(title string, tags []string, body string) (*note.Note, error) {
	id, err := s.NewID()
	if err != nil {
		return nil, err
	}
	now := s.now().UTC()
	n := &note.Note{ID: id, Title: title, Tags: tags, Created: now, Updated: now, Body: body}
	if err := s.write(n); err != nil {
		return nil, err
	}
	return n, nil
}

func (s *Store) Load(id string) (*note.Note, error) {
	if err := validateID(id); err != nil {
		return nil, err
	}
	b, err := os.ReadFile(s.notePath(id))
	if err != nil {
		return nil, err
	}
	return note.Decode(b)
}

func (s *Store) Update(id string, title *string, tags []string, body string) (*note.Note, error) {
	if err := validateID(id); err != nil {
		return nil, err
	}
	n, err := s.Load(id)
	if err != nil {
		return nil, err
	}
	if title != nil {
		n.Title = *title
	}
	if tags != nil {
		n.Tags = tags
	}
	n.Body = body
	n.Updated = s.now().UTC()
	if err := s.write(n); err != nil {
		return nil, err
	}
	return n, nil
}

func (s *Store) Delete(id string) error {
	if err := validateID(id); err != nil {
		return err
	}
	if err := os.Remove(s.notePath(id)); err != nil && !os.IsNotExist(err) {
		return err
	}
	state, err := s.loadState()
	if err != nil {
		return err
	}
	changed := false
	if _, ok := state[id]; ok {
		delete(state, id)
		changed = true
	}
	if _, ok := state[failedPrefix+id]; ok {
		delete(state, failedPrefix+id)
		changed = true
	}
	if changed {
		return s.saveState(state)
	}
	return nil
}

func (s *Store) MarkDeleted(id string) error {
	if err := validateID(id); err != nil {
		return err
	}
	if err := os.Remove(s.notePath(id)); err != nil && !os.IsNotExist(err) {
		return err
	}
	state, err := s.loadState()
	if err != nil {
		return err
	}
	state[failedPrefix+id] = "1"
	return s.saveState(state)
}

func (s *Store) List() ([]*note.Note, error) {
	entries, err := os.ReadDir(filepath.Join(s.root, notesDir))
	if err != nil {
		return nil, err
	}
	var out []*note.Note
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".json") {
			continue
		}
		b, err := os.ReadFile(filepath.Join(s.root, notesDir, e.Name()))
		if err != nil {
			return nil, err
		}
		n, err := note.Decode(b)
		if err != nil {
			return nil, err
		}
		out = append(out, n)
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].Pinned != out[j].Pinned {
			return out[i].Pinned
		}
		return out[i].Updated.After(out[j].Updated)
	})
	return out, nil
}

func (s *Store) ListMeta() ([]*note.NoteMeta, error) {
	entries, err := os.ReadDir(filepath.Join(s.root, notesDir))
	if err != nil {
		return nil, err
	}
	var out []*note.NoteMeta
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".json") {
			continue
		}
		b, err := os.ReadFile(filepath.Join(s.root, notesDir, e.Name()))
		if err != nil {
			return nil, err
		}
		n, err := note.Decode(b)
		if err != nil {
			return nil, err
		}
		out = append(out, n.Meta())
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].Pinned != out[j].Pinned {
			return out[i].Pinned
		}
		return out[i].Updated.After(out[j].Updated)
	})
	return out, nil
}

func (s *Store) SearchMeta(query string) ([]*note.NoteMeta, error) {
	q := strings.TrimSpace(query)
	if q == "" {
		return s.ListMeta()
	}
	needle := strings.ToLower(q)
	entries, err := os.ReadDir(filepath.Join(s.root, notesDir))
	if err != nil {
		return nil, err
	}
	var out []*note.NoteMeta
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".json") {
			continue
		}
		b, err := os.ReadFile(filepath.Join(s.root, notesDir, e.Name()))
		if err != nil {
			return nil, err
		}
		n, err := note.Decode(b)
		if err != nil {
			return nil, err
		}
		title := note.FirstLineTitle(n.Body)
		if title == "" {
			title = n.Title
		}
		if strings.Contains(strings.ToLower(title), needle) ||
			strings.Contains(strings.ToLower(n.Body), needle) {
			out = append(out, n.Meta())
		}
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].Pinned != out[j].Pinned {
			return out[i].Pinned
		}
		return out[i].Updated.After(out[j].Updated)
	})
	return out, nil
}

func (s *Store) Duplicate(id string) (*note.Note, error) {
	if err := validateID(id); err != nil {
		return nil, err
	}
	src, err := s.Load(id)
	if err != nil {
		return nil, err
	}
	return s.Create(src.Title, append([]string{}, src.Tags...), src.Body)
}

func (s *Store) SetPinned(id string, pinned bool) error {
	if err := validateID(id); err != nil {
		return err
	}
	n, err := s.Load(id)
	if err != nil {
		return err
	}
	n.Pinned = pinned
	return s.write(n)
}

func (s *Store) SetFileType(id, ft string) error {
	if err := validateID(id); err != nil {
		return err
	}
	n, err := s.Load(id)
	if err != nil {
		return err
	}
	n.FileType = ft
	return s.write(n)
}

func (s *Store) SetTrashed(id string, trashed bool) error {
	if err := validateID(id); err != nil {
		return err
	}
	n, err := s.Load(id)
	if err != nil {
		return err
	}
	n.Trashed = trashed
	if trashed {
		t := s.now().UTC()
		n.TrashedAt = &t
	} else {
		n.TrashedAt = nil
	}
	return s.write(n)
}

const TrashRetention = 90 * 24 * time.Hour

func (s *Store) PurgeExpiredTrash() ([]string, error) {
	notes, err := s.List()
	if err != nil {
		return nil, err
	}
	cutoff := s.now().UTC().Add(-TrashRetention)
	var purged []string
	for _, n := range notes {
		if !n.Trashed || n.TrashedAt == nil {
			continue
		}
		if n.TrashedAt.Before(cutoff) {
			if err := s.Delete(n.ID); err != nil {
				return purged, err
			}
			purged = append(purged, n.ID)
		}
	}
	return purged, nil
}

func (s *Store) Put(n *note.Note) error {
	if err := validateID(n.ID); err != nil {
		return err
	}
	return s.write(n)
}

func (s *Store) write(n *note.Note) error {
	b, err := n.Encode()
	if err != nil {
		return err
	}
	path := s.notePath(n.ID)
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, b, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

func (s *Store) loadState() (map[string]string, error) {
	b, err := os.ReadFile(filepath.Join(s.root, stateFile))
	if err != nil {
		if os.IsNotExist(err) {
			return map[string]string{}, nil
		}
		return nil, err
	}
	m := map[string]string{}
	if err := json.Unmarshal(b, &m); err != nil {
		return nil, err
	}
	return m, nil
}

func (s *Store) saveState(m map[string]string) error {
	b, err := json.MarshalIndent(m, "", "  ")
	if err != nil {
		return err
	}
	path := filepath.Join(s.root, stateFile)
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, b, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

func (s *Store) SHA(id string) (string, bool, error) {
	state, err := s.loadState()
	if err != nil {
		return "", false, err
	}
	sha, ok := state[id]
	return sha, ok, nil
}

func (s *Store) SetSHA(id, sha string) error {
	state, err := s.loadState()
	if err != nil {
		return err
	}
	state[id] = sha
	return s.saveState(state)
}

func (s *Store) DelSHA(id string) error {
	state, err := s.loadState()
	if err != nil {
		return err
	}
	if _, ok := state[id]; !ok {
		return nil
	}
	delete(state, id)
	return s.saveState(state)
}

const failedPrefix = "__failed__/"

func (s *Store) SetFailed(id string) error {
	return s.SetSHA(failedPrefix+id, "1")
}

func (s *Store) ClearFailed(id string) error {
	return s.DelSHA(failedPrefix + id)
}

func (s *Store) LoadFailed() ([]string, error) {
	state, err := s.loadState()
	if err != nil {
		return nil, err
	}
	var out []string
	for k := range state {
		if strings.HasPrefix(k, failedPrefix) {
			out = append(out, strings.TrimPrefix(k, failedPrefix))
		}
	}
	return out, nil
}
