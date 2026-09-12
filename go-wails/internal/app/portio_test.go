package app

import (
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"
)

func TestImportDirReadsMdAndTxtOnly(t *testing.T) {
	a := testApp(t)
	src := t.TempDir()
	must := func(name, body string) {
		if err := os.WriteFile(filepath.Join(src, name), []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	must("a.md", "# Alpha\nbody a")
	must("b.txt", "Beta first line\nrest")
	must("c.MD", "Case md upper")
	must("d.TXT", "Case txt upper")
	must("e.png", "ignored image")
	must("readme", "no extension ignored")
	if err := os.Mkdir(filepath.Join(src, "sub"), 0o755); err != nil {
		t.Fatal(err)
	}
	must("sub/nested.md", "should not be picked up")

	ids, err := a.ImportDir(src)
	if err != nil {
		t.Fatalf("import: %v", err)
	}
	if len(ids) != 4 {
		t.Fatalf("expected 4 imports (md, txt, .MD, .TXT), got %d", len(ids))
	}
}

func TestImportDirUsesDirBasenameAsTag(t *testing.T) {
	a := testApp(t)
	parent := t.TempDir()
	dir := filepath.Join(parent, "医案")
	if err := os.Mkdir(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "one.md"), []byte("body"), 0o644); err != nil {
		t.Fatal(err)
	}
	ids, err := a.ImportDir(dir)
	if err != nil {
		t.Fatalf("import: %v", err)
	}
	if len(ids) != 1 {
		t.Fatalf("got %d ids", len(ids))
	}
	loaded, err := a.LoadNote(ids[0])
	if err != nil {
		t.Fatalf("load: %v", err)
	}
	if len(loaded.Tags) != 1 || loaded.Tags[0] != "医案" {
		t.Errorf("tags = %v, want [医案]", loaded.Tags)
	}
	if loaded.Body != "body" {
		t.Errorf("body = %q", loaded.Body)
	}
}

func TestImportFilesHasNoTags(t *testing.T) {
	a := testApp(t)
	src := t.TempDir()
	p1 := filepath.Join(src, "a.md")
	p2 := filepath.Join(src, "b.txt")
	os.WriteFile(p1, []byte("hello"), 0o644)
	os.WriteFile(p2, []byte("world"), 0o644)

	ids, err := a.ImportFiles([]string{p1, p2, filepath.Join(src, "no.png")})
	if err != nil {
		t.Fatalf("import: %v", err)
	}
	if len(ids) != 2 {
		t.Fatalf("expected 2 imported (png dropped), got %d", len(ids))
	}
	for _, id := range ids {
		n, err := a.LoadNote(id)
		if err != nil {
			t.Fatal(err)
		}
		if len(n.Tags) != 0 {
			t.Errorf("file import should be tagless, got %v", n.Tags)
		}
	}
}

func TestExportViewAllOnlyNonTrashed(t *testing.T) {
	a := testApp(t)
	a.CreateNote("", []string{"x"}, "kept 1")
	n2, _ := a.CreateNote("", nil, "kept 2")
	trashed, _ := a.CreateNote("", nil, "gone")
	_ = n2
	if err := a.SetTrashed(trashed.ID, true); err != nil {
		t.Fatal(err)
	}
	dest := t.TempDir()
	n, err := a.ExportView(dest, ExportView{Kind: ExportAll}, FormatMD)
	if err != nil {
		t.Fatalf("export: %v", err)
	}
	if n != 2 {
		t.Errorf("expected 2 exported, got %d", n)
	}
	names := listDir(t, filepath.Join(dest, "All"))
	if len(names) != 2 {
		t.Errorf("expected 2 files in All/, got %d: %v", len(names), names)
	}
	for _, name := range names {
		if !strings.HasSuffix(name, ".md") {
			t.Errorf("expected .md, got %s", name)
		}
	}
}

func TestExportViewTrashOnlyTrashed(t *testing.T) {
	a := testApp(t)
	kept, _ := a.CreateNote("", nil, "still here")
	trashed, _ := a.CreateNote("", nil, "gone body")
	_ = kept
	a.SetTrashed(trashed.ID, true)

	dest := t.TempDir()
	n, err := a.ExportView(dest, ExportView{Kind: ExportTrash}, FormatTXT)
	if err != nil {
		t.Fatalf("export: %v", err)
	}
	if n != 1 {
		t.Errorf("expected 1 exported, got %d", n)
	}
	names := listDir(t, filepath.Join(dest, "Trash"))
	if len(names) != 1 {
		t.Fatalf("expected 1 file in Trash/, got %d", len(names))
	}
	if !strings.HasSuffix(names[0], ".txt") {
		t.Errorf("expected .txt, got %s", names[0])
	}
	body, _ := os.ReadFile(filepath.Join(dest, "Trash", names[0]))
	if string(body) != "gone body" {
		t.Errorf("body = %q", body)
	}
}

func TestExportViewUntaggedExcludesTagsAndTrash(t *testing.T) {
	a := testApp(t)
	a.CreateNote("", []string{"x"}, "has tag")
	a.CreateNote("", nil, "no tag kept")
	trashed, _ := a.CreateNote("", nil, "no tag trashed")
	a.SetTrashed(trashed.ID, true)

	dest := t.TempDir()
	n, err := a.ExportView(dest, ExportView{Kind: ExportUntagged}, FormatMD)
	if err != nil {
		t.Fatalf("export: %v", err)
	}
	if n != 1 {
		t.Errorf("expected 1, got %d", n)
	}
	names := listDir(t, filepath.Join(dest, "Untagged"))
	if len(names) != 1 {
		t.Fatalf("expected 1 file, got %d", len(names))
	}
}

func TestExportViewTagPicksMatchingNonTrashed(t *testing.T) {
	a := testApp(t)
	a.CreateNote("", []string{"医案", "笔记"}, "kept 1")
	a.CreateNote("", []string{"医案"}, "kept 2")
	a.CreateNote("", []string{"其他"}, "no match")
	trashed, _ := a.CreateNote("", []string{"医案"}, "trashed match")
	a.SetTrashed(trashed.ID, true)

	dest := t.TempDir()
	n, err := a.ExportView(dest, ExportView{Kind: ExportTag, Tag: "医案"}, FormatMD)
	if err != nil {
		t.Fatalf("export: %v", err)
	}
	if n != 2 {
		t.Errorf("expected 2, got %d", n)
	}
	names := listDir(t, filepath.Join(dest, "医案"))
	if len(names) != 2 {
		t.Fatalf("expected 2 files, got %d: %v", len(names), names)
	}
}

func TestExportDuplicateTitlesGetSuffixes(t *testing.T) {
	a := testApp(t)
	a.CreateNote("", nil, "Same Title\nbody one")
	a.CreateNote("", nil, "Same Title\nbody two")

	dest := t.TempDir()
	n, err := a.ExportView(dest, ExportView{Kind: ExportAll}, FormatMD)
	if err != nil {
		t.Fatalf("export: %v", err)
	}
	if n != 2 {
		t.Errorf("expected 2, got %d", n)
	}
	names := listDir(t, filepath.Join(dest, "All"))
	sort.Strings(names)
	if len(names) != 2 {
		t.Fatalf("expected 2 files, got %d: %v", len(names), names)
	}
	if names[0] != "Same Title-2.md" && names[1] != "Same Title-2.md" {
		t.Errorf("expected one file with -2 suffix, got %v", names)
	}
}

func TestSafeNameStripsFilesystemMetacharsAndControlChars(t *testing.T) {
	cases := map[string]string{
		"plain":              "plain",
		"has/slash":          "has_slash",
		"back\\slash":        "back_slash",
		"colon:in:name":      "colon_in_name",
		"quote\"and*star":    "quote_and_star",
		"pipe|less<greater>": "pipe_less_greater_",
		"tab\there":          "tab_here",
		"null\x00byte":       "null_byte",
		"  trim spaces  ":    "trim spaces",
		"trailing.dots...":   "trailing.dots",
		"question?mark":      "question_mark",
	}
	for in, want := range cases {
		if got := safeName(in); got != want {
			t.Errorf("safeName(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestSafeNameCapsLength(t *testing.T) {
	in := strings.Repeat("a", 200)
	got := safeName(in)
	if len([]rune(got)) != 100 {
		t.Errorf("length %d, want 100", len([]rune(got)))
	}
}

func listDir(t *testing.T, dir string) []string {
	t.Helper()
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatalf("readdir: %v", err)
	}
	names := make([]string, 0, len(entries))
	for _, e := range entries {
		if !e.IsDir() {
			names = append(names, e.Name())
		}
	}
	return names
}

func TestStripMarkdownHeadings(t *testing.T) {
	cases := []struct{ in, want string }{
		{"# Hello", "Hello"},
		{"## Sub", "Sub"},
		{"###### Six", "Six"},
		{"####### Seven hashes stays", "####### Seven hashes stays"},
		{"# Title ##", "Title"},
		{"Setext title\n===", "Setext title"},
		{"Setext sub\n---", "Setext sub"},
	}
	for _, c := range cases {
		if got := stripMarkdown(c.in); got != c.want {
			t.Errorf("stripMarkdown(%q) = %q, want %q", c.in, got, c.want)
		}
	}
}

func TestStripMarkdownInlineFormatting(t *testing.T) {
	cases := []struct{ in, want string }{
		{"**bold** here", "bold here"},
		{"__bold__ here", "bold here"},
		{"*italic* here", "italic here"},
		{"a _italic_ b", "a italic b"},
		{"~~gone~~", "gone"},
		{"`code` inline", "code inline"},
		{"***boldital***", "boldital"},
		{"nested **b*i*b** case", "nested bib case"},
	}
	for _, c := range cases {
		if got := stripMarkdown(c.in); got != c.want {
			t.Errorf("stripMarkdown(%q) = %q, want %q", c.in, got, c.want)
		}
	}
}

func TestStripMarkdownLinksAndImages(t *testing.T) {
	cases := []struct{ in, want string }{
		{"[text](https://x.com)", "text"},
		{"click [here](https://y.com) now", "click here now"},
		{"![alt text](img.png)", "alt text"},
		{"[ref][id]", "ref"},
		{"<https://example.com>", "https://example.com"},
		{"<mailto:a@b.c>", "mailto:a@b.c"},
	}
	for _, c := range cases {
		if got := stripMarkdown(c.in); got != c.want {
			t.Errorf("stripMarkdown(%q) = %q, want %q", c.in, got, c.want)
		}
	}
}

func TestStripMarkdownLists(t *testing.T) {
	in := "- one\n* two\n+ three\n1. four\n2) five"
	want := "one\ntwo\nthree\nfour\nfive"
	if got := stripMarkdown(in); got != want {
		t.Errorf("stripMarkdown(%q) = %q, want %q", in, got, want)
	}
}

func TestStripMarkdownBlockquote(t *testing.T) {
	in := "> quoted line\n>> nested\ntext"
	want := "quoted line\nnested\ntext"
	if got := stripMarkdown(in); got != want {
		t.Errorf("stripMarkdown(%q) = %q, want %q", in, got, want)
	}
}

func TestStripMarkdownFencedCodeUntouched(t *testing.T) {
	in := "```go\nfunc F() {\n\t// **not italic**\n}\n```"
	want := "func F() {\n\t// **not italic**\n}"
	if got := stripMarkdown(in); got != want {
		t.Errorf("stripMarkdown(%q) = %q, want %q", in, got, want)
	}
}

func TestStripMarkdownHorizontalRule(t *testing.T) {
	in := "before\n---\nafter\n***\nend"
	want := "before\nafter\nend"
	if got := stripMarkdown(in); got != want {
		t.Errorf("stripMarkdown(%q) = %q, want %q", in, got, want)
	}
}

func TestStripMarkdownTableSeparator(t *testing.T) {
	in := "| A | B |\n|---|---|\n| 1 | 2 |"
	want := "| A | B |\n| 1 | 2 |"
	if got := stripMarkdown(in); got != want {
		t.Errorf("stripMarkdown(%q) = %q, want %q", in, got, want)
	}
}

func TestStripMarkdownEscapedChars(t *testing.T) {
	in := `\*not italic\* and \[not link\]`
	want := "*not italic* and [not link]"
	if got := stripMarkdown(in); got != want {
		t.Errorf("stripMarkdown(%q) = %q, want %q", in, got, want)
	}
}

func TestStripMarkdownHTMLTags(t *testing.T) {
	in := "<b>keep text</b> and <br/> line"
	want := "keep text and  line"
	if got := stripMarkdown(in); got != want {
		t.Errorf("stripMarkdown(%q) = %q, want %q", in, got, want)
	}
}

func TestExportTXTStripsMarkdownWhileMDKeepsIt(t *testing.T) {
	a := testApp(t)
	body := "# Title\n\n**bold** and [link](https://x)"
	a.CreateNote("", nil, body)

	destMD := t.TempDir()
	if _, err := a.ExportView(destMD, ExportView{Kind: ExportAll}, FormatMD); err != nil {
		t.Fatal(err)
	}
	mdFiles := listDir(t, filepath.Join(destMD, "All"))
	mdBody, _ := os.ReadFile(filepath.Join(destMD, "All", mdFiles[0]))
	if string(mdBody) != body {
		t.Errorf("md export mutated body: %q", mdBody)
	}

	destTXT := t.TempDir()
	if _, err := a.ExportView(destTXT, ExportView{Kind: ExportAll}, FormatTXT); err != nil {
		t.Fatal(err)
	}
	txtFiles := listDir(t, filepath.Join(destTXT, "All"))
	txtBody, _ := os.ReadFile(filepath.Join(destTXT, "All", txtFiles[0]))
	want := "Title\n\nbold and link"
	if string(txtBody) != want {
		t.Errorf("txt export = %q, want %q", txtBody, want)
	}
}

func TestImportUsesFirstLineAsTitle(t *testing.T) {
	a := testApp(t)
	src := t.TempDir()
	path := filepath.Join(src, "a.md")
	if err := os.WriteFile(path, []byte("# Alpha\nbody"), 0o644); err != nil {
		t.Fatal(err)
	}
	ids, err := a.ImportFiles([]string{path})
	if err != nil {
		t.Fatalf("import: %v", err)
	}
	n, err := a.LoadNote(ids[0])
	if err != nil {
		t.Fatalf("load: %v", err)
	}
	if n.Title != "Alpha" {
		t.Errorf("Title = %q, want Alpha", n.Title)
	}
}
