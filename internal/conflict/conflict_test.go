package conflict

import (
	"strings"
	"testing"
)

func TestIdentical(t *testing.T) {
	s := "line1\nline2\n"
	if got := Merge(s, s); got != s {
		t.Errorf("identical merge = %q", got)
	}
}

func TestPureAdditionRemote(t *testing.T) {
	local := "line1\nline2\n"
	remote := "line1\nline2\nline3\n"
	got := Merge(local, remote)
	if HasMarkers(got) {
		t.Errorf("pure addition should not produce markers: %q", got)
	}
	if !strings.Contains(got, "line3") {
		t.Errorf("remote addition lost: %q", got)
	}
}

func TestPureAdditionLocal(t *testing.T) {
	local := "line1\nline2\nlineX\n"
	remote := "line1\nline2\n"
	got := Merge(local, remote)
	if HasMarkers(got) {
		t.Errorf("pure local addition should not produce markers: %q", got)
	}
	if !strings.Contains(got, "lineX") {
		t.Errorf("local addition lost: %q", got)
	}
}

func TestSameLineConflict(t *testing.T) {
	local := "line1\nAAA\nline3\n"
	remote := "line1\nBBB\nline3\n"
	got := Merge(local, remote)
	if !HasMarkers(got) {
		t.Fatalf("expected markers: %q", got)
	}
	for _, m := range []string{markerLocal, markerSep, markerRemote} {
		if !strings.Contains(got, m) {
			t.Errorf("marker %q missing in: %q", m, got)
		}
	}
	li := strings.Index(got, "AAA")
	ri := strings.Index(got, "BBB")
	if li < 0 || ri < 0 || li > ri {
		t.Errorf("local should precede remote: %q", got)
	}
	if !strings.Contains(got, "line1") || !strings.Contains(got, "line3") {
		t.Errorf("common lines lost: %q", got)
	}
}

func TestMarkerLinesStandalone(t *testing.T) {
	local := "x\nAAA\n"
	remote := "x\nBBB\n"
	got := Merge(local, remote)
	lines := strings.Split(strings.TrimSuffix(got, "\n"), "\n")
	found := false
	for _, l := range lines {
		if l == markerLocal {
			found = true
		}
	}
	if !found {
		t.Errorf("marker not standalone line: %q", got)
	}
}

func TestMultipleConflictSegments(t *testing.T) {
	local := "a\nX1\nb\nY1\nc\n"
	remote := "a\nX2\nb\nY2\nc\n"
	got := Merge(local, remote)
	if strings.Count(got, markerLocal) != 2 {
		t.Errorf("expected 2 conflict segments: %q", got)
	}
}

func TestChineseContent(t *testing.T) {
	local := "标题\n本地内容\n结尾\n"
	remote := "标题\n服务器内容\n结尾\n"
	got := Merge(local, remote)
	if !HasMarkers(got) {
		t.Fatalf("expected markers for chinese conflict: %q", got)
	}
	if !strings.Contains(got, "本地内容") || !strings.Contains(got, "服务器内容") {
		t.Errorf("chinese content lost: %q", got)
	}
}

func TestEmptyLocal(t *testing.T) {
	got := Merge("", "remote\n")
	if HasMarkers(got) {
		t.Errorf("empty local vs remote should be pure addition: %q", got)
	}
	if !strings.Contains(got, "remote") {
		t.Errorf("remote lost: %q", got)
	}
}

func TestEmptyRemote(t *testing.T) {
	got := Merge("local\n", "")
	if HasMarkers(got) {
		t.Errorf("local vs empty remote should be pure addition: %q", got)
	}
	if !strings.Contains(got, "local") {
		t.Errorf("local lost: %q", got)
	}
}

func TestConflictAtEOF(t *testing.T) {
	local := "line1\nAAA\n"
	remote := "line1\nBBB\n"
	got := Merge(local, remote)
	if !HasMarkers(got) {
		t.Errorf("expected markers at eof: %q", got)
	}
}

func TestHasMarkersDetection(t *testing.T) {
	if HasMarkers("clean\ntext\n") {
		t.Error("clean text flagged")
	}
	if !HasMarkers("a\n" + markerLocal + "\nb\n") {
		t.Error("marker not detected")
	}
	if HasMarkers("this <<<<<<< inline not a marker\n") {
		t.Error("inline marker-like text wrongly flagged")
	}
}
