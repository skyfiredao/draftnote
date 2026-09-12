package app

import (
	"draftnote/internal/local"
	"draftnote/internal/note"
	"draftnote/internal/sync"
	"draftnote/internal/syncclient"
)

type Config struct {
	Root          string
	BaseURL       string
	Owner         string
	Repo          string
	Username      string
	Token         string
	UseAPI        bool
	OnNoteChanged func(id string)
}

type App struct {
	store      *local.Store
	syncer     *sync.Syncer
	configured bool
}

func New(cfg Config) (*App, error) {
	store, err := local.Open(cfg.Root)
	if err != nil {
		return nil, err
	}
	client := syncclient.New(cfg.BaseURL, cfg.Owner, cfg.Repo, cfg.Username, cfg.Token, cfg.UseAPI)
	syncer := &sync.Syncer{Store: store, Client: client, OnNoteChanged: cfg.OnNoteChanged}
	_ = syncer.LoadFailed()
	return &App{
		store:      store,
		syncer:     syncer,
		configured: cfg.BaseURL != "",
	}, nil
}

func (a *App) ListNotes() ([]*note.NoteMeta, error) {
	metas, err := a.store.ListMeta()
	if err != nil {
		return nil, err
	}
	a.applySyncFailed(metas)
	return metas, nil
}

func (a *App) applySyncFailed(metas []*note.NoteMeta) {
	if !a.configured {
		return
	}
	failed := a.syncer.FailedIDs()
	if len(failed) == 0 {
		return
	}
	for _, m := range metas {
		if failed[m.ID] {
			m.SyncFailed = true
		}
	}
}

func (a *App) AllTags() ([]string, error) {
	metas, err := a.store.ListMeta()
	if err != nil {
		return nil, err
	}
	return note.AllMetaTags(metas), nil
}

func (a *App) NotesByTag(tag string) ([]*note.NoteMeta, error) {
	metas, err := a.store.ListMeta()
	if err != nil {
		return nil, err
	}
	filtered := note.FilterMeta(metas, tag)
	a.applySyncFailed(filtered)
	return filtered, nil
}

func (a *App) SearchNotes(query string) ([]*note.NoteMeta, error) {
	metas, err := a.store.SearchMeta(query)
	if err != nil {
		return nil, err
	}
	a.applySyncFailed(metas)
	return metas, nil
}

func (a *App) LoadNote(id string) (*note.Note, error) {
	return a.store.Load(id)
}

func (a *App) CreateNote(title string, tags []string, body string) (*note.Note, error) {
	n, err := a.store.Create(title, tags, body)
	if err != nil {
		return nil, err
	}
	a.syncer.MarkUnsynced(n.ID)
	return n, nil
}

func (a *App) UpdateNote(id, title string, tags []string, body string) (*note.Note, error) {
	n, err := a.store.Update(id, &title, tags, body)
	if err != nil {
		return nil, err
	}
	a.syncer.MarkUnsynced(id)
	return n, nil
}

func (a *App) DeleteNote(id string) error {
	if err := a.store.MarkDeleted(id); err != nil {
		return err
	}
	a.syncer.MarkUnsynced(id)
	return nil
}

func (a *App) DuplicateNote(id string) (*note.Note, error) {
	return a.store.Duplicate(id)
}

func (a *App) SetPinned(id string, pinned bool) error {
	return a.store.SetPinned(id, pinned)
}

func (a *App) SetNoteFileType(id, ft string) error {
	switch ft {
	case "", "md", "sh", "json":
	default:
		return nil
	}
	if err := a.store.SetFileType(id, ft); err != nil {
		return err
	}
	a.syncer.MarkUnsynced(id)
	return nil
}

func (a *App) SetTrashed(id string, trashed bool) error {
	return a.store.SetTrashed(id, trashed)
}

func (a *App) PurgeExpiredTrash() ([]string, error) {
	return a.store.PurgeExpiredTrash()
}

func (a *App) PushNote(id string) (sync.Result, error) {
	return a.syncer.Push(id)
}

func (a *App) PushDelete(id string) error {
	return a.syncer.PushDelete(id)
}

func (a *App) Pull() ([]string, error) {
	return a.syncer.Pull()
}

func (a *App) Validate() (int, error) {
	if err := a.syncer.Validate(); err != nil {
		return 0, err
	}
	metas, err := a.store.ListMeta()
	if err != nil {
		return 0, err
	}
	return len(metas), nil
}

func (a *App) GetServerRemote() (string, error) {
	return a.syncer.Client.GetRemote()
}

func (a *App) RecloneServer(newURL string) error {
	return a.syncer.Client.PostReclone(newURL)
}

func notePath(id string) string {
	return "notes/" + id + ".json"
}

type Revision struct {
	SHA     string `json:"sha"`
	Author  string `json:"author"`
	Date    string `json:"date"`
	Subject string `json:"subject"`
}

func (a *App) RevisionList(id string, page, perPage int) ([]Revision, error) {
	if page <= 0 {
		page = 1
	}
	if perPage <= 0 {
		perPage = 30
	}
	commits, err := a.syncer.Client.FileHistory(notePath(id), page, perPage)
	if err != nil {
		return nil, err
	}
	out := make([]Revision, 0, len(commits))
	for _, c := range commits {
		out = append(out, Revision{
			SHA:     c.SHA,
			Author:  c.AuthorName,
			Date:    c.Date,
			Subject: c.Subject,
		})
	}
	return out, nil
}

func (a *App) RevisionDiff(id, sha string) (string, error) {
	return a.syncer.Client.CommitPatch(sha, notePath(id))
}

func (a *App) RevisionBody(id, sha string) (string, error) {
	content, _, err := a.syncer.Client.GetAtRef(notePath(id), sha)
	if err != nil {
		return "", err
	}
	if content == "" {
		return "", nil
	}
	n, err := note.Decode([]byte(content))
	if err != nil {
		return content, nil
	}
	return n.Body, nil
}

func (a *App) RestoreRevision(id, sha string) error {
	content, _, err := a.syncer.Client.GetAtRef(notePath(id), sha)
	if err != nil {
		return err
	}
	n, err := note.Decode([]byte(content))
	if err != nil {
		return err
	}
	if _, err := a.store.Update(id, &n.Title, n.Tags, n.Body); err != nil {
		return err
	}
	a.syncer.MarkUnsynced(id)
	_, err = a.syncer.Push(id)
	return err
}
