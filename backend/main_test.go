package main

import (
	"testing"
)

func TestNormalizePort(t *testing.T) {
	cases := map[string]string{
		"8015":         ":8015",
		":8015":        ":8015",
		"":             ":8080",
		"0.0.0.0:8080": "0.0.0.0:8080",
		"  9000  ":     ":9000",
		"not-a-number": "not-a-number",
	}
	for in, want := range cases {
		if got := normalizePort(in); got != want {
			t.Errorf("normalizePort(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestResolveAddrPrecedence(t *testing.T) {
	t.Setenv("DRAFTNOTE_ADDR", "")
	t.Setenv("DRAFTNOTE_PORT", "")
	if got := resolveAddr("", ""); got != ":8080" {
		t.Errorf("default = %q, want :8080", got)
	}

	t.Setenv("DRAFTNOTE_PORT", "9090")
	if got := resolveAddr("", ""); got != ":9090" {
		t.Errorf("env port = %q, want :9090", got)
	}

	if got := resolveAddr("", "7070"); got != ":7070" {
		t.Errorf("flag port over env = %q, want :7070", got)
	}

	t.Setenv("DRAFTNOTE_ADDR", "10.0.0.1:5000")
	if got := resolveAddr("", "7070"); got != "10.0.0.1:5000" {
		t.Errorf("env addr over flag port = %q, want 10.0.0.1:5000", got)
	}

	if got := resolveAddr(":6060", "7070"); got != ":6060" {
		t.Errorf("flag addr top precedence = %q, want :6060", got)
	}
}

func TestFirstNonEmpty(t *testing.T) {
	if got := firstNonEmpty("", "", "x"); got != "x" {
		t.Errorf("firstNonEmpty = %q, want x", got)
	}
	if got := firstNonEmpty("a", "b"); got != "a" {
		t.Errorf("firstNonEmpty = %q, want a", got)
	}
	if got := firstNonEmpty("", ""); got != "" {
		t.Errorf("firstNonEmpty = %q, want empty", got)
	}
}
