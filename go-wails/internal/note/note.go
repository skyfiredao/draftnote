package note

import (
	"encoding/json"
	"sort"
	"strings"
	"time"
)

type Note struct {
	ID        string     `json:"id"`
	Title     string     `json:"title"`
	Tags      []string   `json:"tags"`
	Created   time.Time  `json:"created"`
	Updated   time.Time  `json:"updated"`
	Body      string     `json:"body"`
	Pinned    bool       `json:"pinned,omitempty"`
	Trashed   bool       `json:"trashed,omitempty"`
	TrashedAt *time.Time `json:"trashed_at,omitempty"`
	FileType  string     `json:"filetype,omitempty"`
}

type NoteMeta struct {
	ID          string     `json:"id"`
	Title       string     `json:"title"`
	Tags        []string   `json:"tags"`
	Created     time.Time  `json:"created"`
	Updated     time.Time  `json:"updated"`
	Pinned      bool       `json:"pinned,omitempty"`
	Trashed     bool       `json:"trashed,omitempty"`
	TrashedAt   *time.Time `json:"trashed_at,omitempty"`
	HasConflict bool       `json:"has_conflict,omitempty"`
	SyncFailed  bool       `json:"sync_failed,omitempty"`
	FileType    string     `json:"filetype,omitempty"`
}

func Decode(b []byte) (*Note, error) {
	var n Note
	if err := json.Unmarshal(b, &n); err != nil {
		return nil, err
	}
	return &n, nil
}

func (n *Note) Encode() ([]byte, error) {
	c := *n
	c.Body = normalizeLineEndings(c.Body)
	return json.MarshalIndent(&c, "", "  ")
}

func normalizeLineEndings(s string) string {
	s = strings.ReplaceAll(s, "\r\n", "\n")
	s = strings.ReplaceAll(s, "\r", "\n")
	return s
}

func (n *Note) HasTag(tag string) bool {
	for _, t := range n.Tags {
		if t == tag {
			return true
		}
	}
	return false
}

func FirstLineTitle(body string) string {
	for _, raw := range strings.Split(body, "\n") {
		if strings.TrimSpace(raw) == "" {
			continue
		}
		line := strings.TrimLeft(raw, " \t")
		for len(line) > 0 && line[0] == '#' {
			line = line[1:]
		}
		return strings.TrimSpace(line)
	}
	return ""
}

func (n *Note) Meta() *NoteMeta {
	title := metaTitle(n)
	return &NoteMeta{
		ID:          n.ID,
		Title:       title,
		Tags:        n.Tags,
		Created:     n.Created,
		Updated:     n.Updated,
		Pinned:      n.Pinned,
		Trashed:     n.Trashed,
		TrashedAt:   n.TrashedAt,
		HasConflict: hasConflictMarkers(n.Body),
		FileType:    n.FileType,
	}
}

func metaTitle(n *Note) string {
	switch n.FileType {
	case "json", "sh":
		return n.Title
	default:
		if t := FirstLineTitle(n.Body); t != "" {
			return t
		}
		return n.Title
	}
}

func hasConflictMarkers(body string) bool {
	hasLocal := false
	hasRemote := false
	for _, line := range strings.Split(body, "\n") {
		if line == "<<<<<<< local version" {
			hasLocal = true
		} else if line == ">>>>>>> remote version" {
			hasRemote = true
		}
		if hasLocal && hasRemote {
			return true
		}
	}
	return false
}

func (m *NoteMeta) HasTag(tag string) bool {
	for _, t := range m.Tags {
		if t == tag {
			return true
		}
	}
	return false
}

func Filter(notes []*Note, tag string) []*Note {
	var out []*Note
	for _, n := range notes {
		if n.HasTag(tag) {
			out = append(out, n)
		}
	}
	return out
}

func FilterMeta(metas []*NoteMeta, tag string) []*NoteMeta {
	var out []*NoteMeta
	for _, m := range metas {
		if m.HasTag(tag) {
			out = append(out, m)
		}
	}
	return out
}

func AllTags(notes []*Note) []string {
	seen := map[string]struct{}{}
	for _, n := range notes {
		for _, t := range n.Tags {
			seen[t] = struct{}{}
		}
	}
	out := make([]string, 0, len(seen))
	for t := range seen {
		out = append(out, t)
	}
	sort.Strings(out)
	return out
}

func AllMetaTags(metas []*NoteMeta) []string {
	seen := map[string]struct{}{}
	for _, m := range metas {
		for _, t := range m.Tags {
			seen[t] = struct{}{}
		}
	}
	out := make([]string, 0, len(seen))
	for t := range seen {
		out = append(out, t)
	}
	sort.Strings(out)
	return out
}
