package sync

import (
	"crypto/sha1"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"strings"
	stdsync "sync"

	"draftnote/internal/conflict"
	"draftnote/internal/local"
	"draftnote/internal/note"
	"draftnote/internal/syncclient"
)

const (
	notesDir = "notes"
	headKey  = "__head__"
)

type Syncer struct {
	Store         *local.Store
	Client        *syncclient.Client
	OnNoteChanged func(id string)

	opMu stdsync.Mutex

	failedMu stdsync.Mutex
	failed   map[string]bool
}

type Result struct {
	ID         string
	Conflicted bool
}

func notePath(id string) string { return notesDir + "/" + id + ".json" }

func (s *Syncer) notify(id string) {
	if s.OnNoteChanged != nil {
		s.OnNoteChanged(id)
	}
}

func (s *Syncer) markFailed(id string) {
	s.failedMu.Lock()
	if s.failed == nil {
		s.failed = map[string]bool{}
	}
	s.failed[id] = true
	s.failedMu.Unlock()
	_ = s.Store.SetFailed(id)
}

func (s *Syncer) MarkUnsynced(id string) { s.markFailed(id) }

func (s *Syncer) clearFailed(id string) {
	s.failedMu.Lock()
	delete(s.failed, id)
	s.failedMu.Unlock()
	_ = s.Store.ClearFailed(id)
}

func (s *Syncer) LoadFailed() error {
	ids, err := s.Store.LoadFailed()
	if err != nil {
		return err
	}
	s.failedMu.Lock()
	if s.failed == nil {
		s.failed = map[string]bool{}
	}
	for _, id := range ids {
		s.failed[id] = true
	}
	s.failedMu.Unlock()
	return nil
}

func (s *Syncer) FailedIDs() map[string]bool {
	s.failedMu.Lock()
	defer s.failedMu.Unlock()
	out := make(map[string]bool, len(s.failed))
	for id := range s.failed {
		out[id] = true
	}
	return out
}

func (s *Syncer) isFailed(id string) bool {
	s.failedMu.Lock()
	defer s.failedMu.Unlock()
	return s.failed[id]
}

func (s *Syncer) ensure() error {
	return s.Client.EnsureBranch()
}

func gitBlobSHA(content []byte) string {
	h := sha1.New()
	fmt.Fprintf(h, "blob %d\x00", len(content))
	h.Write(content)
	return hex.EncodeToString(h.Sum(nil))
}

func remoteBodyFromContent(remoteContent string) string {
	if strings.TrimSpace(remoteContent) == "" {
		return ""
	}
	if n, err := note.Decode([]byte(remoteContent)); err == nil {
		return n.Body
	}
	return remoteContent
}

func trimJSON(name string) string {
	if len(name) > 5 && name[len(name)-5:] == ".json" {
		return name[:len(name)-5]
	}
	return name
}

func idFromPath(p string) string {
	if !strings.HasPrefix(p, notesDir+"/") {
		return ""
	}
	name := strings.TrimPrefix(p, notesDir+"/")
	if !strings.HasSuffix(name, ".json") {
		return ""
	}
	id := strings.TrimSuffix(name, ".json")
	if err := local.ValidateID(id); err != nil {
		return ""
	}
	return id
}

func (s *Syncer) localBlobSHA(id string) (encoded []byte, sha string, ok bool, err error) {
	n, lerr := s.Store.Load(id)
	if lerr != nil {
		if os.IsNotExist(lerr) {
			return nil, "", false, nil
		}
		return nil, "", false, lerr
	}
	enc, eerr := n.Encode()
	if eerr != nil {
		return nil, "", false, eerr
	}
	return enc, gitBlobSHA(enc), true, nil
}

func (s *Syncer) Push(id string) (Result, error) {
	s.opMu.Lock()
	defer s.opMu.Unlock()
	if err := s.ensure(); err != nil {
		s.markFailed(id)
		return Result{ID: id}, err
	}
	return s.reconcile(id, "")
}

func (s *Syncer) PushDelete(id string) error {
	s.opMu.Lock()
	defer s.opMu.Unlock()
	if err := s.ensure(); err != nil {
		return err
	}
	_, err := s.reconcile(id, "")
	return err
}

func (s *Syncer) Pull() ([]string, error) {
	res, err := s.SyncAll()
	if err != nil {
		return nil, err
	}
	ids := make([]string, 0, len(res))
	for _, r := range res {
		ids = append(ids, r.ID)
	}
	return ids, nil
}

func (s *Syncer) Validate() error {
	_, err := s.SyncAll()
	return err
}

func (s *Syncer) SyncAll() ([]Result, error) {
	s.opMu.Lock()
	defer s.opMu.Unlock()
	if err := s.ensure(); err != nil {
		return nil, err
	}
	remoteHead, err := s.Client.Head("")
	if err != nil {
		return nil, err
	}
	localHead, _, err := s.Store.SHA(headKey)
	if err != nil {
		return nil, err
	}

	remoteHint := map[string]string{}
	useBootstrap := false
	if localHead == "" {
		useBootstrap = true
	} else if remoteHead != "" && localHead != remoteHead {
		changes, cerr := s.Client.Compare(localHead, remoteHead)
		if cerr != nil {
			useBootstrap = true
		} else {
			for _, c := range changes {
				id := idFromPath(c.Path)
				if id == "" {
					continue
				}
				remoteHint[id] = c.Status
			}
		}
	}

	if useBootstrap {
		entries, lerr := s.Client.List(notesDir)
		if lerr != nil {
			return nil, lerr
		}
		for _, e := range entries {
			if e.Type != "file" {
				continue
			}
			id := trimJSON(e.Name)
			if err := local.ValidateID(id); err != nil {
				continue
			}
			remoteHint[id] = "modified"
		}
		metas, merr := s.Store.ListMeta()
		if merr != nil {
			return nil, merr
		}
		for _, m := range metas {
			s.markFailed(m.ID)
		}
	}

	ids := map[string]bool{}
	for id := range remoteHint {
		ids[id] = true
	}
	for id := range s.FailedIDs() {
		ids[id] = true
	}

	var results []Result
	anyFailed := false
	for id := range ids {
		hint := remoteHint[id]
		r, err := s.reconcile(id, hint)
		if err != nil {
			anyFailed = true
			continue
		}
		results = append(results, r)
	}

	if !anyFailed && len(s.FailedIDs()) == 0 && remoteHead != "" {
		if err := s.Store.SetSHA(headKey, remoteHead); err != nil {
			return results, err
		}
	}
	return results, nil
}

func (s *Syncer) reconcile(id, remoteHint string) (Result, error) {
	if err := local.ValidateID(id); err != nil {
		return Result{ID: id}, err
	}
	baseSHA, _, err := s.Store.SHA(id)
	if err != nil {
		return Result{ID: id}, err
	}
	dirty := s.isFailed(id)
	encoded, localSHA, localExists, err := s.localBlobSHA(id)
	if err != nil {
		return Result{ID: id}, err
	}

	if !localExists {
		return s.reconcileLocalMissing(id, baseSHA, dirty, remoteHint)
	}
	return s.reconcileLocalPresent(id, baseSHA, dirty, remoteHint, encoded, localSHA)
}

func (s *Syncer) reconcileLocalMissing(id, baseSHA string, dirty bool, remoteHint string) (Result, error) {
	if dirty {
		if baseSHA == "" || remoteHint == "removed" {
			s.clearFailed(id)
			_ = s.Store.DelSHA(id)
			return Result{ID: id}, nil
		}
		if remoteHint == "added" || remoteHint == "modified" {
			return s.pullDown(id)
		}
		if err := s.Client.Delete(notePath(id), baseSHA, "delete "+id); err != nil {
			if errors.Is(err, syncclient.ErrConflict) {
				return s.pullDown(id)
			}
			s.markFailed(id)
			return Result{ID: id}, err
		}
		s.clearFailed(id)
		_ = s.Store.DelSHA(id)
		return Result{ID: id}, nil
	}
	if remoteHint == "added" || remoteHint == "modified" {
		return s.pullDown(id)
	}
	if remoteHint == "removed" {
		_ = s.Store.DelSHA(id)
		return Result{ID: id}, nil
	}
	return Result{ID: id}, nil
}

func (s *Syncer) reconcileLocalPresent(id, baseSHA string, dirty bool, remoteHint string, encoded []byte, localSHA string) (Result, error) {
	effectiveDirty := dirty || baseSHA == "" || localSHA != baseSHA
	if remoteHint == "removed" {
		if !effectiveDirty {
			if err := s.Store.Delete(id); err != nil {
				return Result{ID: id}, err
			}
			s.notify(id)
			return Result{ID: id}, nil
		}
		newSHA, err := s.Client.Put(notePath(id), string(encoded), "", "recreate "+id)
		if err != nil {
			s.markFailed(id)
			return Result{ID: id}, err
		}
		if err := s.Store.SetSHA(id, newSHA); err != nil {
			return Result{ID: id}, err
		}
		s.clearFailed(id)
		return Result{ID: id, Conflicted: conflict.HasMarkers(mustLoadBody(s, id))}, nil
	}

	if remoteHint == "added" || remoteHint == "modified" {
		return s.mergeWithRemote(id, baseSHA, effectiveDirty, encoded, localSHA)
	}

	if !effectiveDirty {
		if dirty {
			s.clearFailed(id)
		} else {
		}
		return Result{ID: id}, nil
	}
	if conflict.HasMarkers(mustLoadBody(s, id)) {
		return Result{ID: id, Conflicted: true}, nil
	}
	newSHA, err := s.Client.Put(notePath(id), string(encoded), baseSHA, "update "+id)
	if err == nil {
		if e := s.Store.SetSHA(id, newSHA); e != nil {
			return Result{ID: id}, e
		}
		s.clearFailed(id)
		return Result{ID: id}, nil
	}
	if !errors.Is(err, syncclient.ErrConflict) {
		s.markFailed(id)
		return Result{ID: id}, err
	}
	return s.mergeWithRemote(id, baseSHA, effectiveDirty, encoded, localSHA)
}

func (s *Syncer) mergeWithRemote(id, baseSHA string, dirty bool, encoded []byte, localSHA string) (Result, error) {
	remoteContent, remoteSHA, err := s.Client.Get(notePath(id))
	if err != nil {
		s.markFailed(id)
		return Result{ID: id}, err
	}
	if remoteContent == "" && remoteSHA == "" {
		if !dirty {
			return Result{ID: id}, nil
		}
		newSHA, perr := s.Client.Put(notePath(id), string(encoded), "", "recreate "+id)
		if perr != nil {
			s.markFailed(id)
			return Result{ID: id}, perr
		}
		if err := s.Store.SetSHA(id, newSHA); err != nil {
			return Result{ID: id}, err
		}
		s.clearFailed(id)
		return Result{ID: id}, nil
	}
	remoteBlob := remoteSHA
	if remoteBlob == localSHA {
		if err := s.Store.SetSHA(id, remoteBlob); err != nil {
			return Result{ID: id}, err
		}
		s.clearFailed(id)
		return Result{ID: id}, nil
	}
	if baseSHA != "" && remoteBlob == baseSHA {
		if !dirty {
			return Result{ID: id}, nil
		}
		newSHA, perr := s.Client.Put(notePath(id), string(encoded), baseSHA, "update "+id)
		if perr == nil {
			if err := s.Store.SetSHA(id, newSHA); err != nil {
				return Result{ID: id}, err
			}
			s.clearFailed(id)
			return Result{ID: id}, nil
		}
		if !errors.Is(perr, syncclient.ErrConflict) {
			s.markFailed(id)
			return Result{ID: id}, perr
		}
	}
	if !dirty && (baseSHA == "" || localSHA == baseSHA) {
		n, derr := note.Decode([]byte(remoteContent))
		if derr != nil {
			s.markFailed(id)
			return Result{ID: id}, derr
		}
		if err := s.Store.Put(n); err != nil {
			s.markFailed(id)
			return Result{ID: id}, err
		}
		if err := s.Store.SetSHA(id, remoteBlob); err != nil {
			return Result{ID: id}, err
		}
		s.notify(id)
		return Result{ID: id}, nil
	}
	localNote, lerr := s.Store.Load(id)
	if lerr != nil {
		s.markFailed(id)
		return Result{ID: id}, lerr
	}
	remoteBody := remoteBodyFromContent(remoteContent)
	merged := conflict.Merge(localNote.Body, remoteBody)
	if _, err := s.Store.Update(id, nil, nil, merged); err != nil {
		s.markFailed(id)
		return Result{ID: id}, err
	}
	if err := s.Store.SetSHA(id, remoteBlob); err != nil {
		return Result{ID: id}, err
	}
	s.notify(id)
	mergedNote, lerr2 := s.Store.Load(id)
	if lerr2 != nil {
		s.markFailed(id)
		return Result{ID: id, Conflicted: conflict.HasMarkers(merged)}, lerr2
	}
	mergedEncoded, encErr := mergedNote.Encode()
	if encErr != nil {
		s.markFailed(id)
		return Result{ID: id, Conflicted: conflict.HasMarkers(merged)}, encErr
	}
	newSHA, perr := s.Client.Put(notePath(id), string(mergedEncoded), remoteBlob, "conflict "+id)
	if perr != nil {
		s.markFailed(id)
		return Result{ID: id, Conflicted: conflict.HasMarkers(merged)}, nil
	}
	if err := s.Store.SetSHA(id, newSHA); err != nil {
		return Result{ID: id, Conflicted: conflict.HasMarkers(merged)}, err
	}
	if conflict.HasMarkers(merged) {
		return Result{ID: id, Conflicted: true}, nil
	}
	s.clearFailed(id)
	return Result{ID: id}, nil
}

func (s *Syncer) pullDown(id string) (Result, error) {
	content, sha, err := s.Client.Get(notePath(id))
	if err != nil {
		s.markFailed(id)
		return Result{ID: id}, err
	}
	if content == "" && sha == "" {
		s.clearFailed(id)
		_ = s.Store.DelSHA(id)
		return Result{ID: id}, nil
	}
	n, err := note.Decode([]byte(content))
	if err != nil {
		s.markFailed(id)
		return Result{ID: id}, err
	}
	if err := s.Store.Put(n); err != nil {
		s.markFailed(id)
		return Result{ID: id}, err
	}
	if err := s.Store.SetSHA(id, sha); err != nil {
		return Result{ID: id}, err
	}
	s.clearFailed(id)
	s.notify(id)
	return Result{ID: id}, nil
}

func mustLoadBody(s *Syncer, id string) string {
	n, err := s.Store.Load(id)
	if err != nil {
		return ""
	}
	return n.Body
}
