package app

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"unicode"
	"unicode/utf8"

	"draftnote/internal/note"
)

type ExportViewKind int

const (
	ExportAll ExportViewKind = iota
	ExportTrash
	ExportUntagged
	ExportTag
)

type ExportView struct {
	Kind ExportViewKind
	Tag  string
}

type ExportFormat string

const (
	FormatMD  ExportFormat = "md"
	FormatTXT ExportFormat = "txt"
)

func isImportableExt(path string) bool {
	ext := strings.ToLower(filepath.Ext(path))
	return ext == ".md" || ext == ".txt" || ext == ".sh" || ext == ".json"
}

func detectFileType(path string) string {
	switch strings.ToLower(filepath.Ext(path)) {
	case ".sh":
		return "sh"
	case ".json":
		return "json"
	default:
		return ""
	}
}

func shellBodyWithTitleComment(body, filename string) string {
	for _, raw := range strings.Split(body, "\n") {
		line := strings.TrimSpace(raw)
		if line == "" {
			continue
		}
		if strings.HasPrefix(line, "#") {
			return body
		}
		break
	}
	prefix := "# " + filename + "\n"
	if body == "" {
		return prefix
	}
	return prefix + body
}

func prettyJSON(body string) string {
	var buf bytes.Buffer
	if err := json.Indent(&buf, []byte(body), "", "  "); err != nil {
		return body
	}
	return buf.String()
}

func (a *App) ImportDir(dir string) ([]string, error) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, err
	}
	base := filepath.Base(dir)
	var tags []string
	if base != "" && base != "." && base != string(filepath.Separator) {
		tags = []string{base}
	}
	var paths []string
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		p := filepath.Join(dir, e.Name())
		if isImportableExt(p) {
			paths = append(paths, p)
		}
	}
	return a.importFiles(paths, tags)
}

func (a *App) ImportFiles(paths []string) ([]string, error) {
	var eligible []string
	for _, p := range paths {
		if isImportableText(p) {
			eligible = append(eligible, p)
		}
	}
	return a.importFiles(eligible, nil)
}

func isImportableText(p string) bool {
	info, err := os.Stat(p)
	if err != nil || info.IsDir() {
		return false
	}
	b, err := os.ReadFile(p)
	if err != nil {
		return false
	}
	if bytes.IndexByte(b, 0) >= 0 {
		return false
	}
	return utf8.Valid(b)
}

func (a *App) importFiles(paths []string, tags []string) ([]string, error) {
	ids := make([]string, 0, len(paths))
	for _, p := range paths {
		b, err := os.ReadFile(p)
		if err != nil {
			return ids, fmt.Errorf("read %s: %w", p, err)
		}
		ft := detectFileType(p)
		filename := filepath.Base(p)
		body := string(b)
		var title string
		switch ft {
		case "sh":
			body = shellBodyWithTitleComment(body, filename)
			title = filename
		case "json":
			body = prettyJSON(body)
			title = filename
		default:
			title = note.FirstLineTitle(body)
			if title == "" {
				title = filename
			}
		}
		n, err := a.store.Create(title, tags, body)
		if err != nil {
			return ids, err
		}
		if ft != "" {
			if err := a.store.SetFileType(n.ID, ft); err != nil {
				return ids, err
			}
		}
		a.syncer.MarkUnsynced(n.ID)
		ids = append(ids, n.ID)
	}
	return ids, nil
}

func (a *App) ExportView(destDir string, view ExportView, format ExportFormat) (int, error) {
	if destDir == "" {
		return 0, errors.New("empty destination")
	}
	metas, err := a.store.ListMeta()
	if err != nil {
		return 0, err
	}
	subdir, filter, err := viewSpec(view)
	if err != nil {
		return 0, err
	}
	ext, err := extForFormat(format)
	if err != nil {
		return 0, err
	}
	sub := safeName(subdir)
	if sub == "" {
		sub = "Export"
	}
	outDir := filepath.Join(destDir, sub)
	if err := os.MkdirAll(outDir, 0o755); err != nil {
		return 0, err
	}
	used := map[string]struct{}{}
	count := 0
	for _, m := range metas {
		if !filter(m) {
			continue
		}
		n, err := a.store.Load(m.ID)
		if err != nil {
			return count, err
		}
		base := safeName(m.Title)
		if base == "" {
			base = "note"
		}
		fileExt := ext
		body := n.Body
		switch n.FileType {
		case "sh":
			fileExt = ".sh"
		case "json":
			fileExt = ".json"
		default:
			if format == FormatTXT {
				body = stripMarkdown(body)
			}
		}
		name := uniqueName(used, base, fileExt)
		used[name] = struct{}{}
		if err := os.WriteFile(filepath.Join(outDir, name), []byte(body), 0o644); err != nil {
			return count, err
		}
		count++
	}
	return count, nil
}

func viewSpec(view ExportView) (string, func(*note.NoteMeta) bool, error) {
	switch view.Kind {
	case ExportAll:
		return "All", func(m *note.NoteMeta) bool { return !m.Trashed }, nil
	case ExportTrash:
		return "Trash", func(m *note.NoteMeta) bool { return m.Trashed }, nil
	case ExportUntagged:
		return "Untagged", func(m *note.NoteMeta) bool { return !m.Trashed && len(m.Tags) == 0 }, nil
	case ExportTag:
		tag := view.Tag
		return tag, func(m *note.NoteMeta) bool {
			if m.Trashed {
				return false
			}
			for _, t := range m.Tags {
				if t == tag {
					return true
				}
			}
			return false
		}, nil
	default:
		return "", nil, errors.New("unknown export view")
	}
}

func extForFormat(f ExportFormat) (string, error) {
	switch f {
	case FormatMD:
		return ".md", nil
	case FormatTXT:
		return ".txt", nil
	default:
		return "", fmt.Errorf("unknown format: %q", f)
	}
}

func safeName(s string) string {
	var b strings.Builder
	for _, r := range s {
		switch {
		case r < 0x20:
			b.WriteByte('_')
		case r == '/' || r == '\\' || r == ':' || r == '*' || r == '?' ||
			r == '"' || r == '<' || r == '>' || r == '|':
			b.WriteByte('_')
		default:
			b.WriteRune(r)
		}
	}
	out := b.String()
	out = strings.TrimFunc(out, func(r rune) bool {
		return unicode.IsSpace(r) || r == '.'
	})
	runes := []rune(out)
	if len(runes) > 100 {
		out = string(runes[:100])
	}
	return out
}

func uniqueName(used map[string]struct{}, base, ext string) string {
	name := base + ext
	if _, ok := used[name]; !ok {
		return name
	}
	for i := 2; ; i++ {
		candidate := fmt.Sprintf("%s-%d%s", base, i, ext)
		if _, ok := used[candidate]; !ok {
			return candidate
		}
	}
}

var (
	reImage     = regexp.MustCompile(`!\[([^\]]*)\]\([^)]*\)`)
	reInlineImg = regexp.MustCompile(`!\[([^\]]*)\]\[[^\]]*\]`)
	reLink      = regexp.MustCompile(`\[([^\]]*)\]\(([^)]*)\)`)
	reRefLink   = regexp.MustCompile(`\[([^\]]*)\]\[[^\]]*\]`)
	reAutolink  = regexp.MustCompile(`<((?:https?|ftp|mailto):[^>]+)>`)
	reInlineCd  = regexp.MustCompile("`+([^`]+?)`+")
	reBoldStar  = regexp.MustCompile(`\*\*\*?(.+?)\*\*\*?`)
	reBoldUS    = regexp.MustCompile(`__(.+?)__`)
	reItalicS   = regexp.MustCompile(`\*([^*\n]+?)\*`)
	reItalicU   = regexp.MustCompile(`\b_([^_\n]+?)_\b`)
	reStrike    = regexp.MustCompile(`~~([^~\n]+?)~~`)
	reHTMLTag   = regexp.MustCompile(`</?[a-zA-Z][^>]*>`)
	reTableSep  = regexp.MustCompile(`^\s*\|?[\s:\-]*\|[\s:\-|]*\|?\s*$`)
)

func stripMarkdown(s string) string {
	lines := strings.Split(s, "\n")
	out := make([]string, 0, len(lines))
	inFence := false
	for i, line := range lines {
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "```") || strings.HasPrefix(trimmed, "~~~") {
			inFence = !inFence
			continue
		}
		if inFence {
			out = append(out, line)
			continue
		}
		if isHorizontalRule(trimmed) {
			continue
		}
		if isSetextUnderline(trimmed) && len(out) > 0 && strings.TrimSpace(out[len(out)-1]) != "" {

			_ = i
			continue
		}
		if reTableSep.MatchString(line) {

			continue
		}
		line = stripATXHeading(line)
		line = stripBlockquotePrefix(line)
		line = stripListMarker(line)
		line = stripInline(line)
		out = append(out, line)
	}
	return strings.Join(out, "\n")
}

func isHorizontalRule(trimmed string) bool {
	if len(trimmed) < 3 {
		return false
	}
	c := trimmed[0]
	if c != '-' && c != '*' && c != '_' {
		return false
	}
	for i := 0; i < len(trimmed); i++ {
		if trimmed[i] == ' ' || trimmed[i] == '\t' {
			continue
		}
		if trimmed[i] != c {
			return false
		}
	}
	return true
}

func isSetextUnderline(trimmed string) bool {
	if len(trimmed) < 2 {
		return false
	}
	c := trimmed[0]
	if c != '=' && c != '-' {
		return false
	}
	for i := 0; i < len(trimmed); i++ {
		if trimmed[i] != c {
			return false
		}
	}
	return true
}

func stripATXHeading(line string) string {

	i := 0
	for i < len(line) && (line[i] == ' ' || line[i] == '\t') {
		i++
	}
	hashes := 0
	for i+hashes < len(line) && line[i+hashes] == '#' && hashes < 6 {
		hashes++
	}
	if hashes == 0 {
		return line
	}
	rest := line[i+hashes:]
	if rest == "" {
		return ""
	}
	if rest[0] != ' ' && rest[0] != '\t' {
		return line
	}
	rest = strings.TrimLeft(rest, " \t")

	rest = strings.TrimRight(rest, " \t")
	rest = strings.TrimRight(rest, "#")
	rest = strings.TrimRight(rest, " \t")
	return rest
}

func stripBlockquotePrefix(line string) string {
	for {
		i := 0
		for i < len(line) && (line[i] == ' ' || line[i] == '\t') {
			i++
		}
		if i < len(line) && line[i] == '>' {
			j := i + 1
			if j < len(line) && (line[j] == ' ' || line[j] == '\t') {
				j++
			}
			line = line[j:]
			continue
		}
		return line
	}
}

func stripListMarker(line string) string {
	i := 0
	for i < len(line) && (line[i] == ' ' || line[i] == '\t') {
		i++
	}
	if i >= len(line) {
		return line
	}

	c := line[i]
	if (c == '-' || c == '*' || c == '+') && i+1 < len(line) && (line[i+1] == ' ' || line[i+1] == '\t') {
		return line[:i] + strings.TrimLeft(line[i+2:], " \t")
	}

	j := i
	for j < len(line) && line[j] >= '0' && line[j] <= '9' {
		j++
	}
	if j > i && j < len(line) && (line[j] == '.' || line[j] == ')') &&
		j+1 < len(line) && (line[j+1] == ' ' || line[j+1] == '\t') {
		return line[:i] + strings.TrimLeft(line[j+2:], " \t")
	}
	return line
}

func stripInline(line string) string {
	line = hideEscapes(line)
	line = reImage.ReplaceAllString(line, "$1")
	line = reInlineImg.ReplaceAllString(line, "$1")
	line = reLink.ReplaceAllString(line, "$1")
	line = reRefLink.ReplaceAllString(line, "$1")
	line = reAutolink.ReplaceAllString(line, "$1")
	line = reInlineCd.ReplaceAllString(line, "$1")
	line = reBoldStar.ReplaceAllString(line, "$1")
	line = reBoldUS.ReplaceAllString(line, "$1")
	line = reItalicS.ReplaceAllString(line, "$1")
	line = reItalicU.ReplaceAllString(line, "$1")
	line = reStrike.ReplaceAllString(line, "$1")
	line = reHTMLTag.ReplaceAllString(line, "")
	line = unhideEscapes(line)
	return line
}

func isEscapable(c byte) bool {
	switch c {
	case '\\', '`', '*', '_', '{', '}', '[', ']', '(', ')', '#', '+', '-', '.', '!', '~', '<', '>', '|':
		return true
	}
	return false
}

func hideEscapes(s string) string {
	var b strings.Builder
	b.Grow(len(s))
	for i := 0; i < len(s); i++ {
		if s[i] == '\\' && i+1 < len(s) && isEscapable(s[i+1]) {
			b.WriteRune(0xE800 + rune(s[i+1]))
			i++
			continue
		}
		b.WriteByte(s[i])
	}
	return b.String()
}

func unhideEscapes(s string) string {
	var b strings.Builder
	b.Grow(len(s))
	for _, r := range s {
		if r >= 0xE800 && r <= 0xE87F {
			b.WriteRune(r - 0xE800)
			continue
		}
		b.WriteRune(r)
	}
	return b.String()
}
