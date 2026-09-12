package main

import (
	"errors"
	"os"
	"strings"
	"testing"
)

func TestParseRepoURL(t *testing.T) {
	cases := []struct {
		name   string
		url    string
		useAPI bool
		base   string
		owner  string
		repo   string
	}{
		{"github api mode", "https://github.com/foo/bar", true, "https://api.github.com", "foo", "bar"},
		{"github with .git suffix", "https://github.com/foo/bar.git", true, "https://api.github.com", "foo", "bar"},
		{"github hosted mode uses host as base", "https://github.com/foo/bar", false, "https://github.com", "foo", "bar"},
		{"self-hosted api mode keeps host", "https://git.example.com/foo/bar", true, "https://git.example.com", "foo", "bar"},
		{"self-hosted hosted mode keeps host", "https://git.example.com/foo/bar", false, "https://git.example.com", "foo", "bar"},
		{"self-hosted with port", "https://git.example.com:8443/foo/bar", true, "https://git.example.com:8443", "foo", "bar"},
		{"www.github.com treated as github", "https://www.github.com/foo/bar", true, "https://api.github.com", "foo", "bar"},
		{"http scheme accepted", "http://internal.local/foo/bar", true, "http://internal.local", "foo", "bar"},
		{"extra path segments ignored", "https://github.com/foo/bar/tree/main", true, "https://api.github.com", "foo", "bar"},
		{"trailing slash accepted", "https://github.com/foo/bar/", true, "https://api.github.com", "foo", "bar"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			base, owner, repo, err := parseRepoURL(c.url, c.useAPI)
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if base != c.base || owner != c.owner || repo != c.repo {
				t.Errorf("got base=%q owner=%q repo=%q, want %q %q %q", base, owner, repo, c.base, c.owner, c.repo)
			}
		})
	}
}

func TestParseRepoURLInvalid(t *testing.T) {
	bad := []string{
		"",
		"   ",
		"not a url",
		"github.com/foo/bar",
		"ftp://github.com/foo/bar",
		"https://github.com/foo",
		"https://github.com",
		"https:///foo/bar",
	}
	for _, s := range bad {
		t.Run(s, func(t *testing.T) {
			_, _, _, err := parseRepoURL(s, true)
			if err == nil {
				t.Errorf("expected error for %q", s)
			}
			if !errors.Is(err, ErrInvalidRepoURL) {
				t.Errorf("expected ErrInvalidRepoURL, got %v", err)
			}
		})
	}
}

func TestParseExportView(t *testing.T) {
	cases := []struct {
		in      string
		wantErr bool
	}{
		{"all", false},
		{"trash", false},
		{"untagged", false},
		{"tag:医案", false},
		{"tag:", true},
		{"tag", true},
		{"", true},
		{"whatever", true},
	}
	for _, c := range cases {
		_, err := parseExportView(c.in)
		if (err != nil) != c.wantErr {
			t.Errorf("parseExportView(%q) err=%v wantErr=%v", c.in, err, c.wantErr)
		}
	}
}

func TestParseExportFormat(t *testing.T) {
	if _, err := parseExportFormat("md"); err != nil {
		t.Errorf("md should parse, got %v", err)
	}
	if _, err := parseExportFormat("txt"); err != nil {
		t.Errorf("txt should parse, got %v", err)
	}
	if _, err := parseExportFormat("doc"); err == nil {
		t.Error("doc should be rejected")
	}
}

func TestMapValidateError(t *testing.T) {
	cfg := Config{BaseURL: "https://git.example.com"}
	cases := []struct {
		name string
		in   string
		want string
	}{
		{"401", "create tree: status 401: bad token", "Auth rejected: check username/token"},
		{"403", "create tree: status 403: sso required", "Forbidden: token lacks repo access"},
		{"repo 404", "create tree: status 404: Not Found", "Repo not found: check owner/repo in URL"},
		{"5xx", "create tree: status 502: Bad Gateway", "Server error: 502"},
		{"create ref", "create ref draftnote: status 422: something", "Cannot create draftnote branch: create ref draftnote: status 422: something"},
		{"no host", "Get \"https://git.example.com/repos/o/r\": dial tcp: lookup git.example.com: no such host", "Cannot reach server: git.example.com"},
		{"other passthrough", "some unrelated error", "some unrelated error"},
	}
	for _, c := range cases {
		got := mapValidateError(cfg, errorsNew(c.in))
		if got != c.want {
			t.Errorf("%s: got %q, want %q", c.name, got, c.want)
		}
	}
}

func errorsNew(s string) error { return errors.New(s) }

func TestSaveConfigEncryptsTokenAndLoadRestoresIt(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("HOME", dir)

	c := Config{
		RepoURL:             "https://github.com/foo/bar",
		Username:            "alice",
		Token:               "s3cret",
		UseAPI:              true,
		BaseURL:             "https://api.github.com",
		Owner:               "foo",
		Repo:                "bar",
		SyncIntervalSeconds: 5,
	}
	if err := saveConfig(c); err != nil {
		t.Fatalf("save: %v", err)
	}
	raw, err := os.ReadFile(configPath())
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(raw), "s3cret") {
		t.Errorf("plaintext token found on disk: %s", raw)
	}
	if !strings.Contains(string(raw), "token_secret") {
		t.Errorf("token_secret field missing: %s", raw)
	}

	got := loadConfig()
	if got.Token != "s3cret" {
		t.Errorf("token round-trip: got %q, want s3cret", got.Token)
	}
	if got.RepoURL != c.RepoURL || got.BaseURL != c.BaseURL {
		t.Errorf("other fields corrupted: %+v", got)
	}
}

func TestConfigureServerURLOverridesBase(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("HOME", dir)

	b := NewBridge()
	err := b.Configure(
		"https://github.com/foo/bar",
		"http://10.0.1.244:8015/",
		"alice",
		"tok",
		false,
		5,
		false,
	)
	if err != nil {
		t.Fatalf("configure: %v", err)
	}
	got := loadConfig()
	if got.BaseURL != "http://10.0.1.244:8015" {
		t.Errorf("BaseURL = %q, want server URL", got.BaseURL)
	}
	if got.Owner != "foo" || got.Repo != "bar" {
		t.Errorf("owner/repo = %q/%q, expected from Repo URL", got.Owner, got.Repo)
	}
	if got.ServerURL != "http://10.0.1.244:8015" {
		t.Errorf("ServerURL not persisted: %q", got.ServerURL)
	}
}

func TestConfigureEmptyServerURLKeepsDerivedBase(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("HOME", dir)

	b := NewBridge()
	if err := b.Configure(
		"https://github.com/foo/bar",
		"",
		"",
		"tok",
		true,
		5,
		false,
	); err != nil {
		t.Fatalf("configure: %v", err)
	}
	got := loadConfig()
	if got.BaseURL != "https://api.github.com" {
		t.Errorf("BaseURL = %q, want api.github.com", got.BaseURL)
	}
	if got.ServerURL != "" {
		t.Errorf("ServerURL should be empty, got %q", got.ServerURL)
	}
}

func TestConfigureServerURLPreservesPort(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("HOME", dir)

	b := NewBridge()
	err := b.Configure(
		"http://10.0.1.244:8015/dw/draftnote.git",
		"http://10.0.1.244:8015",
		"alice",
		"tok",
		false,
		5,
		false,
	)
	if err != nil {
		t.Fatalf("configure: %v", err)
	}
	got := loadConfig()
	if got.BaseURL != "http://10.0.1.244:8015" {
		t.Errorf("BaseURL = %q, want http://10.0.1.244:8015", got.BaseURL)
	}
	if got.ServerURL != "http://10.0.1.244:8015" {
		t.Errorf("ServerURL = %q", got.ServerURL)
	}
}

func TestConfigureRepoURLPortInfersBase(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("HOME", dir)

	b := NewBridge()
	if err := b.Configure(
		"http://10.0.1.244:8015/dw/draftnote.git",
		"",
		"alice",
		"tok",
		false,
		5,
		false,
	); err != nil {
		t.Fatalf("configure: %v", err)
	}
	got := loadConfig()
	if got.BaseURL != "http://10.0.1.244:8015" {
		t.Errorf("BaseURL = %q, want http://10.0.1.244:8015", got.BaseURL)
	}
	if got.Owner != "dw" || got.Repo != "draftnote" {
		t.Errorf("owner/repo = %q/%q, want dw/draftnote", got.Owner, got.Repo)
	}
}

func TestConfigureServerURLWithPortAndTrailingPath(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("HOME", dir)

	b := NewBridge()
	if err := b.Configure(
		"https://github.com/foo/bar",
		"http://10.0.1.244:8015/",
		"",
		"tok",
		true,
		5,
		false,
	); err != nil {
		t.Fatalf("configure: %v", err)
	}
	got := loadConfig()
	if got.BaseURL != "http://10.0.1.244:8015" {
		t.Errorf("BaseURL = %q, want http://10.0.1.244:8015 (trailing slash trimmed)", got.BaseURL)
	}
}
