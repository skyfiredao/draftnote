package note

import (
	"testing"
	"time"
)

func sample(id, title string, tags ...string) *Note {
	return &Note{
		ID:      id,
		Title:   title,
		Tags:    tags,
		Created: time.Date(2026, 9, 1, 14, 30, 22, 0, time.UTC),
		Updated: time.Date(2026, 9, 1, 15, 4, 11, 0, time.UTC),
		Body:    "# 正文\n内容",
	}
}

func TestEncodeDecodeRoundTrip(t *testing.T) {
	in := sample("n1", "大柴胡汤心得", "医案", "少阳病")
	b, err := in.Encode()
	if err != nil {
		t.Fatalf("encode: %v", err)
	}
	out, err := Decode(b)
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	if out.ID != in.ID || out.Title != in.Title || out.Body != in.Body {
		t.Errorf("mismatch: %+v vs %+v", out, in)
	}
	if len(out.Tags) != 2 || out.Tags[0] != "医案" || out.Tags[1] != "少阳病" {
		t.Errorf("tags: %v", out.Tags)
	}
	if !out.Created.Equal(in.Created) || !out.Updated.Equal(in.Updated) {
		t.Errorf("time: %v %v", out.Created, out.Updated)
	}
}

func TestDecodeInvalid(t *testing.T) {
	if _, err := Decode([]byte("not json")); err == nil {
		t.Fatal("expected error")
	}
}

func TestHasTagAndFilter(t *testing.T) {
	a := sample("a", "A", "医案", "太阳病")
	b := sample("b", "B", "笔记")
	c := sample("c", "C", "医案")
	if !a.HasTag("太阳病") || a.HasTag("少阳病") {
		t.Error("HasTag wrong")
	}
	got := Filter([]*Note{a, b, c}, "医案")
	if len(got) != 2 || got[0].ID != "a" || got[1].ID != "c" {
		t.Errorf("Filter 医案 = %v", ids(got))
	}
	if len(Filter([]*Note{a, b, c}, "无此tag")) != 0 {
		t.Error("Filter absent should be empty")
	}
}

func TestAllTagsSortedUnique(t *testing.T) {
	a := sample("a", "A", "医案", "太阳病")
	b := sample("b", "B", "医案", "笔记")
	got := AllTags([]*Note{a, b})
	want := []string{"医案", "太阳病", "笔记"}
	if len(got) != len(want) {
		t.Fatalf("AllTags = %v want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("AllTags[%d] = %q want %q", i, got[i], want[i])
		}
	}
}

func ids(ns []*Note) []string {
	out := make([]string, len(ns))
	for i, n := range ns {
		out[i] = n.ID
	}
	return out
}

func TestMetaHasConflict(t *testing.T) {
	cases := []struct {
		name string
		body string
		want bool
	}{
		{"clean", "just body\nno markers\n", false},
		{"only local marker", "<<<<<<< local version\ntext\n", false},
		{"only remote marker", "text\n>>>>>>> remote version\n", false},
		{
			"both markers",
			"line\n<<<<<<< local version\nlocal\n=======\nremote\n>>>>>>> remote version\ntail\n",
			true,
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			n := &Note{ID: "n", Body: c.body}
			if got := n.Meta().HasConflict; got != c.want {
				t.Errorf("HasConflict = %v want %v", got, c.want)
			}
		})
	}
}

func TestEncodeNormalizesLineEndings(t *testing.T) {
	n := &Note{ID: "n1", Title: "t", Body: "a\r\nb\rc\n"}
	b, err := n.Encode()
	if err != nil {
		t.Fatalf("encode: %v", err)
	}
	if bytesContainsCR(b) {
		t.Errorf("encoded bytes still contain CR: %q", string(b))
	}
	if n.Body != "a\r\nb\rc\n" {
		t.Errorf("in-memory body must not be mutated: %q", n.Body)
	}
}

func bytesContainsCR(b []byte) bool {
	for _, c := range b {
		if c == '\r' {
			return true
		}
	}
	return false
}
