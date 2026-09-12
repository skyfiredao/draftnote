package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"time"

	"draftnote/internal/app"
	"draftnote/internal/note"
	"draftnote/internal/secret"
	"draftnote/internal/sync"

	"github.com/wailsapp/wails/v2/pkg/runtime"
)

const defaultSyncIntervalSeconds = 10

type Bridge struct {
	ctx context.Context
	app *app.App
	cfg Config
}

func NewBridge() *Bridge {
	return &Bridge{}
}

type Config struct {
	RepoURL              string `json:"repo_url"`
	ServerURL            string `json:"server_url,omitempty"`
	Username             string `json:"username"`
	Token                string `json:"-"`
	TokenSecret          string `json:"token_secret,omitempty"`
	UseAPI               bool   `json:"use_api"`
	BaseURL              string `json:"base_url"`
	Owner                string `json:"owner"`
	Repo                 string `json:"repo"`
	SyncIntervalSeconds  int    `json:"sync_interval_seconds"`
	MismatchAckServerURL string `json:"mismatch_ack_server_url,omitempty"`
}

func (c Config) interval() time.Duration {
	s := c.SyncIntervalSeconds
	if s <= 0 {
		s = defaultSyncIntervalSeconds
	}
	return time.Duration(s) * time.Second
}

func defaultRoot() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return ".draftnote"
	}
	return filepath.Join(home, ".draftnote")
}

func configPath() string {
	return filepath.Join(defaultRoot(), "config.json")
}

func loadConfig() Config {
	var c Config
	b, err := os.ReadFile(configPath())
	if err != nil {
		c.SyncIntervalSeconds = defaultSyncIntervalSeconds
		c.UseAPI = true
		return c
	}
	_ = json.Unmarshal(b, &c)
	if c.SyncIntervalSeconds <= 0 {
		c.SyncIntervalSeconds = defaultSyncIntervalSeconds
	}
	if c.TokenSecret != "" {
		if tok, err := secret.Open(c.TokenSecret); err == nil {
			c.Token = tok
		}
	}
	return c
}

func saveConfig(c Config) error {
	if err := os.MkdirAll(defaultRoot(), 0o755); err != nil {
		return err
	}
	sealed, err := secret.Seal(c.Token)
	if err != nil {
		return err
	}
	c.TokenSecret = sealed
	b, err := json.MarshalIndent(c, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(configPath(), b, 0o600)
}

var ErrInvalidRepoURL = errors.New("invalid repo URL")

func parseRepoURL(repoURL string, useAPI bool) (base, owner, repo string, err error) {
	s := strings.TrimSpace(repoURL)
	s = strings.TrimSuffix(s, ".git")
	if s == "" {
		return "", "", "", ErrInvalidRepoURL
	}
	u, perr := url.Parse(s)
	if perr != nil {
		return "", "", "", ErrInvalidRepoURL
	}
	if u.Scheme != "http" && u.Scheme != "https" {
		return "", "", "", ErrInvalidRepoURL
	}
	if u.Host == "" {
		return "", "", "", ErrInvalidRepoURL
	}
	parts := strings.Split(strings.Trim(u.Path, "/"), "/")
	if len(parts) < 2 || parts[0] == "" || parts[1] == "" {
		return "", "", "", ErrInvalidRepoURL
	}
	owner = parts[0]
	repo = parts[1]
	host := strings.ToLower(u.Host)
	if useAPI && (host == "github.com" || host == "www.github.com") {
		base = "https://api.github.com"
	} else {
		base = u.Scheme + "://" + u.Host
	}
	return base, owner, repo, nil
}

func (b *Bridge) buildApp(cfg Config) error {
	a, err := app.New(app.Config{
		Root:     defaultRoot(),
		BaseURL:  cfg.BaseURL,
		Owner:    cfg.Owner,
		Repo:     cfg.Repo,
		Username: cfg.Username,
		Token:    cfg.Token,
		UseAPI:   cfg.UseAPI,
		OnNoteChanged: func(id string) {
			if b.ctx != nil {
				runtime.EventsEmit(b.ctx, "note-body-changed", id)
			}
		},
	})
	if err != nil {
		return err
	}
	b.app = a
	b.cfg = cfg
	return nil
}

func (b *Bridge) startup(ctx context.Context) {
	b.ctx = ctx
	cfg := loadConfig()
	if err := b.buildApp(cfg); err == nil {
		_, _ = b.app.PurgeExpiredTrash()
	}
	go b.autoSync()
}

func (b *Bridge) autoSync() {
	for {
		interval := b.cfg.interval()
		select {
		case <-b.ctx.Done():
			return
		case <-time.After(interval):
			if b.app == nil || b.cfg.BaseURL == "" {
				continue
			}
			pulled, err := b.app.Pull()
			if err != nil {
				continue
			}
			if len(pulled) > 0 {
				runtime.EventsEmit(b.ctx, "notes-changed")
			}
		}
	}
}

type PublicConfig struct {
	RepoURL             string `json:"repo_url"`
	ServerURL           string `json:"server_url"`
	Username            string `json:"username"`
	UseAPI              bool   `json:"use_api"`
	HasToken            bool   `json:"has_token"`
	SyncIntervalSeconds int    `json:"sync_interval_seconds"`
}

func (b *Bridge) GetConfig() PublicConfig {
	c := loadConfig()
	return PublicConfig{
		RepoURL:             c.RepoURL,
		ServerURL:           c.ServerURL,
		Username:            c.Username,
		UseAPI:              c.UseAPI,
		HasToken:            c.Token != "",
		SyncIntervalSeconds: c.SyncIntervalSeconds,
	}
}

func (b *Bridge) Configure(repoURL, serverURL, username, token string, useAPI bool, syncIntervalSeconds int, keepToken bool) error {
	if syncIntervalSeconds <= 0 {
		syncIntervalSeconds = defaultSyncIntervalSeconds
	}
	base, owner, repo, err := parseRepoURL(repoURL, useAPI)
	if err != nil {
		return fmt.Errorf("parse repo URL: %w", err)
	}
	serverURL = strings.TrimSpace(serverURL)
	if serverURL != "" {
		serverURL = strings.TrimRight(serverURL, "/")
		base = serverURL
	}
	if keepToken {
		token = loadConfig().Token
	}
	cfg := Config{
		RepoURL:             strings.TrimSpace(repoURL),
		ServerURL:           serverURL,
		Username:            strings.TrimSpace(username),
		Token:               token,
		UseAPI:              useAPI,
		BaseURL:             base,
		Owner:               owner,
		Repo:                repo,
		SyncIntervalSeconds: syncIntervalSeconds,
	}
	if err := saveConfig(cfg); err != nil {
		return err
	}
	return b.buildApp(cfg)
}

func (b *Bridge) ListNotes() ([]*note.NoteMeta, error)            { return b.app.ListNotes() }
func (b *Bridge) AllTags() ([]string, error)                      { return b.app.AllTags() }
func (b *Bridge) NotesByTag(tag string) ([]*note.NoteMeta, error) { return b.app.NotesByTag(tag) }
func (b *Bridge) SearchNotes(q string) ([]*note.NoteMeta, error)  { return b.app.SearchNotes(q) }
func (b *Bridge) LoadNote(id string) (*note.Note, error)          { return b.app.LoadNote(id) }

func (b *Bridge) CreateNote(title string, tags []string, body string) (*note.Note, error) {
	return b.app.CreateNote(title, tags, body)
}

func (b *Bridge) UpdateNote(id, title string, tags []string, body string) (*note.Note, error) {
	return b.app.UpdateNote(id, title, tags, body)
}

func (b *Bridge) DeleteNote(id string) error { return b.app.DeleteNote(id) }
func (b *Bridge) DuplicateNote(id string) (*note.Note, error) {
	return b.app.DuplicateNote(id)
}
func (b *Bridge) SetPinned(id string, pinned bool) error   { return b.app.SetPinned(id, pinned) }
func (b *Bridge) SetNoteFileType(id, ft string) error     { return b.app.SetNoteFileType(id, ft) }
func (b *Bridge) SetTrashed(id string, trashed bool) error { return b.app.SetTrashed(id, trashed) }
func (b *Bridge) PurgeExpiredTrash() ([]string, error)     { return b.app.PurgeExpiredTrash() }
func (b *Bridge) PushNote(id string) (sync.Result, error)  { return b.app.PushNote(id) }
func (b *Bridge) PushDelete(id string) error               { return b.app.PushDelete(id) }

func (b *Bridge) RevisionList(id string, page, perPage int) ([]app.Revision, error) {
	if b.app == nil {
		return nil, errors.New("Not configured")
	}
	return b.app.RevisionList(id, page, perPage)
}

func (b *Bridge) RevisionDiff(id, sha string) (string, error) {
	if b.app == nil {
		return "", errors.New("Not configured")
	}
	return b.app.RevisionDiff(id, sha)
}

func (b *Bridge) RevisionBody(id, sha string) (string, error) {
	if b.app == nil {
		return "", errors.New("Not configured")
	}
	return b.app.RevisionBody(id, sha)
}

func (b *Bridge) RestoreRevision(id, sha string) error {
	if b.app == nil {
		return errors.New("Not configured")
	}
	return b.app.RestoreRevision(id, sha)
}

func (b *Bridge) ValidateSync() (string, error) {
	cfg := loadConfig()
	if cfg.RepoURL == "" || cfg.BaseURL == "" || cfg.Owner == "" || cfg.Repo == "" {
		return "", errors.New("Missing repo URL")
	}
	if cfg.Token == "" {
		return "", errors.New("Missing token")
	}
	if !cfg.UseAPI && cfg.Username == "" {
		return "", errors.New("Missing username")
	}
	if b.app == nil {
		return "", errors.New("Not configured")
	}
	count, err := b.app.Validate()
	if err != nil {
		return "", errors.New(mapValidateError(cfg, err))
	}
	return fmt.Sprintf("OK: synced %d notes", count), nil
}

type MismatchInfo struct {
	Mismatch  bool   `json:"mismatch"`
	ServerURL string `json:"server_url"`
}

func (b *Bridge) CheckRemoteMismatch() (MismatchInfo, error) {
	cfg := loadConfig()
	if b.app == nil || cfg.UseAPI || cfg.RepoURL == "" {
		return MismatchInfo{}, nil
	}
	serverURL, err := b.app.GetServerRemote()
	if err != nil {
		return MismatchInfo{}, err
	}
	if serverURL == "" {
		return MismatchInfo{}, nil
	}
	if normalizeRepoURL(serverURL) == normalizeRepoURL(cfg.RepoURL) {
		return MismatchInfo{ServerURL: serverURL}, nil
	}
	if serverURL == cfg.MismatchAckServerURL {
		return MismatchInfo{ServerURL: serverURL}, nil
	}
	return MismatchInfo{Mismatch: true, ServerURL: serverURL}, nil
}

func (b *Bridge) RecloneServer() error {
	cfg := loadConfig()
	if b.app == nil {
		return errors.New("Not configured")
	}
	if cfg.RepoURL == "" {
		return errors.New("Missing repo URL")
	}
	if err := b.app.RecloneServer(cfg.RepoURL); err != nil {
		return err
	}

	if cfg.MismatchAckServerURL != "" {
		cfg.MismatchAckServerURL = ""
		return saveConfig(cfg)
	}
	return nil
}

func (b *Bridge) AcknowledgeRemoteMismatch(serverURL string) error {
	cfg := loadConfig()
	cfg.MismatchAckServerURL = strings.TrimSpace(serverURL)
	return saveConfig(cfg)
}

func normalizeRepoURL(s string) string {
	s = strings.TrimSpace(s)
	s = strings.TrimSuffix(s, ".git")
	s = strings.TrimRight(s, "/")
	return strings.ToLower(s)
}

func mapValidateError(cfg Config, err error) string {
	msg := err.Error()
	lower := strings.ToLower(msg)
	if strings.Contains(lower, "status 401") {
		return "Auth rejected: check username/token"
	}
	if strings.Contains(lower, "status 403") {
		return "Forbidden: token lacks repo access"
	}
	if (strings.Contains(lower, "create tree:") || strings.Contains(lower, "create commit:")) && strings.Contains(lower, "status 404") {
		return "Repo not found: check owner/repo in URL"
	}
	if strings.Contains(lower, "status 5") {
		return "Server error: " + extractStatus(msg)
	}
	if strings.Contains(lower, "create ref") || strings.Contains(lower, "create tree") || strings.Contains(lower, "create commit") {
		return "Cannot create draftnote branch: " + msg
	}
	if strings.Contains(lower, "no such host") || strings.Contains(lower, "dial tcp") || strings.Contains(lower, "connection refused") || strings.Contains(lower, "timeout") {
		host := cfg.BaseURL
		if u, uerr := url.Parse(cfg.BaseURL); uerr == nil && u.Host != "" {
			host = u.Host
		}
		return "Cannot reach server: " + host
	}
	return msg
}

func extractStatus(msg string) string {
	i := strings.Index(msg, "status ")
	if i < 0 {
		return msg
	}
	rest := msg[i+len("status "):]
	end := strings.IndexAny(rest, " :")
	if end < 0 {
		return rest
	}
	return rest[:end]
}

func (b *Bridge) ImportFromDir() (int, error) {
	if b.app == nil {
		return 0, nil
	}
	dir, err := runtime.OpenDirectoryDialog(b.ctx, runtime.OpenDialogOptions{
		Title: "Import Notes from Directory",
	})
	if err != nil {
		return 0, err
	}
	if dir == "" {
		return 0, nil
	}
	ids, err := b.app.ImportDir(dir)
	if err != nil {
		return len(ids), err
	}
	go b.pushImported(ids)
	return len(ids), nil
}

func (b *Bridge) ImportFromFiles() (int, error) {
	if b.app == nil {
		return 0, nil
	}
	paths, err := runtime.OpenMultipleFilesDialog(b.ctx, runtime.OpenDialogOptions{
		Title: "Import Notes",
		Filters: []runtime.FileFilter{
			{DisplayName: "Notes (*.md;*.txt;*.sh;*.json)", Pattern: "*.md;*.txt;*.sh;*.json"},
			{DisplayName: "All Files (*.*)", Pattern: "*.*"},
		},
	})
	if err != nil {
		return 0, err
	}
	if len(paths) == 0 {
		return 0, nil
	}
	ids, err := b.app.ImportFiles(paths)
	if err != nil {
		return len(ids), err
	}
	go b.pushImported(ids)
	return len(ids), nil
}

func (b *Bridge) pushImported(ids []string) {
	for _, id := range ids {
		_, _ = b.app.PushNote(id)
	}
	if len(ids) > 0 {
		runtime.EventsEmit(b.ctx, "notes-changed")
	}
}

func (b *Bridge) ExportView(view, format string) (int, error) {
	if b.app == nil {
		return 0, nil
	}
	dest, err := runtime.OpenDirectoryDialog(b.ctx, runtime.OpenDialogOptions{
		Title:                "Export Notes To",
		CanCreateDirectories: true,
	})
	if err != nil {
		return 0, err
	}
	if dest == "" {
		return 0, nil
	}
	v, err := parseExportView(view)
	if err != nil {
		return 0, err
	}
	f, err := parseExportFormat(format)
	if err != nil {
		return 0, err
	}
	return b.app.ExportView(dest, v, f)
}

func parseExportView(view string) (app.ExportView, error) {
	switch {
	case view == "all":
		return app.ExportView{Kind: app.ExportAll}, nil
	case view == "trash":
		return app.ExportView{Kind: app.ExportTrash}, nil
	case view == "untagged":
		return app.ExportView{Kind: app.ExportUntagged}, nil
	case strings.HasPrefix(view, "tag:"):
		name := strings.TrimPrefix(view, "tag:")
		if name == "" {
			return app.ExportView{}, fmt.Errorf("empty tag name")
		}
		return app.ExportView{Kind: app.ExportTag, Tag: name}, nil
	default:
		return app.ExportView{}, fmt.Errorf("unknown view: %q", view)
	}
}

func parseExportFormat(format string) (app.ExportFormat, error) {
	switch format {
	case "md":
		return app.FormatMD, nil
	case "txt":
		return app.FormatTXT, nil
	default:
		return "", fmt.Errorf("unknown format: %q", format)
	}
}
