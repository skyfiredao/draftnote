package server

import (
	"crypto/subtle"
	"encoding/base64"
	"encoding/json"
	"log"
	"net/http"
	"strconv"
	"strings"

	"draftnote-backend/gitstore"
)

const MaxRequestBytes int64 = 5 << 20

type Server struct {
	Store    *gitstore.Store
	Username string
	Token    string
}

type fileResponse struct {
	Name    string `json:"name"`
	Path    string `json:"path"`
	Type    string `json:"type"`
	SHA     string `json:"sha"`
	Content string `json:"content,omitempty"`
}

type putRequest struct {
	Message string `json:"message"`
	Content string `json:"content"`
	SHA     string `json:"sha"`
}

type deleteRequest struct {
	Message string `json:"message"`
	SHA     string `json:"sha"`
}

type putResponse struct {
	Content struct {
		SHA string `json:"sha"`
	} `json:"content"`
}

func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("/remote", s.auth(s.handleRemote))
	mux.HandleFunc("/repos/", s.auth(s.handleRepo))
	return mux
}

func (s *Server) tokenMatches(got string) bool {
	want := "token " + s.Token
	return subtle.ConstantTimeCompare([]byte(got), []byte(want)) == 1
}

func (s *Server) basicMatches(got string) bool {
	const prefix = "Basic "
	if !strings.HasPrefix(got, prefix) {
		return false
	}
	raw, err := base64.StdEncoding.DecodeString(got[len(prefix):])
	if err != nil {
		return false
	}
	i := strings.IndexByte(string(raw), ':')
	if i < 0 {
		return false
	}
	user := string(raw[:i])
	pass := string(raw[i+1:])
	userOK := subtle.ConstantTimeCompare([]byte(user), []byte(s.Username)) == 1
	passOK := subtle.ConstantTimeCompare([]byte(pass), []byte(s.Token)) == 1
	return userOK && passOK
}

func (s *Server) auth(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if s.Token != "" {
			got := r.Header.Get("Authorization")
			if !s.tokenMatches(got) && !s.basicMatches(got) {
				http.Error(w, "unauthorized", http.StatusUnauthorized)
				return
			}
		}
		next(w, r)
	}
}

func serverError(w http.ResponseWriter, op string, err error) {
	log.Printf("server: %s: %v", sanitizeLog(op), sanitizeLog(err.Error()))
	http.Error(w, "internal error", http.StatusInternalServerError)
}

func clientError(w http.ResponseWriter, op string, err error, status int) {
	log.Printf("server: %s: %v", sanitizeLog(op), sanitizeLog(err.Error()))
	http.Error(w, "bad request", status)
}

func sanitizeLog(s string) string {
	s = strings.ReplaceAll(s, "\r", " ")
	s = strings.ReplaceAll(s, "\n", " ")
	return s
}

func contentsPath(urlPath string) string {
	i := strings.Index(urlPath, "/contents/")
	if i < 0 {
		return ""
	}
	return strings.Trim(urlPath[i+len("/contents/"):], "/")
}

func (s *Server) handleRepo(w http.ResponseWriter, r *http.Request) {
	switch {
	case strings.Contains(r.URL.Path, "/contents/"):
		s.handleContents(w, r)
	case strings.Contains(r.URL.Path, "/compare/"):
		s.handleCompare(w, r)
	case strings.Contains(r.URL.Path, "/commits/") || strings.HasSuffix(r.URL.Path, "/commits"):
		s.handleCommits(w, r)
	default:
		http.NotFound(w, r)
	}
}

func (s *Server) handleCommits(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	ref := commitsRef(r.URL.Path)
	q := r.URL.Query()
	if ref == "" {
		s.commitHistory(w, r, q)
		return
	}
	if q.Get("path") != "" || q.Get("per_page") != "" || q.Get("page") != "" {
		s.commitHistory(w, r, q)
		return
	}
	if isHexSHA(ref) {
		s.commitDetail(w, ref)
		return
	}
	sha, err := s.Store.HeadSHA()
	if err != nil {
		serverError(w, "head", err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]string{"sha": sha})
}

func commitsRef(urlPath string) string {
	i := strings.Index(urlPath, "/commits")
	if i < 0 {
		return ""
	}
	rest := urlPath[i+len("/commits"):]
	return strings.Trim(rest, "/")
}

func isHexSHA(ref string) bool {
	if len(ref) < 7 || len(ref) > 40 {
		return false
	}
	for _, c := range ref {
		if !((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f')) {
			return false
		}
	}
	return true
}

type commitListItem struct {
	SHA    string `json:"sha"`
	Commit struct {
		Message string `json:"message"`
		Author  struct {
			Name string `json:"name"`
			Date string `json:"date"`
		} `json:"author"`
	} `json:"commit"`
}

func (s *Server) commitHistory(w http.ResponseWriter, r *http.Request, q map[string][]string) {
	path := ""
	if v, ok := q["path"]; ok && len(v) > 0 {
		path = strings.Trim(v[0], "/")
	}
	perPage := 30
	if v, ok := q["per_page"]; ok && len(v) > 0 {
		if n, err := strconv.Atoi(v[0]); err == nil && n > 0 {
			perPage = n
		}
	}
	page := 1
	if v, ok := q["page"]; ok && len(v) > 0 {
		if n, err := strconv.Atoi(v[0]); err == nil && n > 0 {
			page = n
		}
	}
	metas, err := s.Store.FileHistory(path, (page-1)*perPage, perPage)
	if err != nil {
		serverError(w, "history", err)
		return
	}
	list := make([]commitListItem, 0, len(metas))
	for _, m := range metas {
		var it commitListItem
		it.SHA = m.SHA
		it.Commit.Message = m.Subject
		it.Commit.Author.Name = m.AuthorName
		it.Commit.Author.Date = m.Date
		list = append(list, it)
	}
	writeJSON(w, http.StatusOK, list)
}

type commitFile struct {
	Filename string `json:"filename"`
	Patch    string `json:"patch,omitempty"`
}

type commitDetailResponse struct {
	SHA   string       `json:"sha"`
	Files []commitFile `json:"files"`
}

func (s *Server) commitDetail(w http.ResponseWriter, sha string) {
	files, err := s.Store.CommitFiles(sha)
	if err != nil {
		serverError(w, "commit files", err)
		return
	}
	resp := commitDetailResponse{SHA: sha, Files: make([]commitFile, 0, len(files))}
	for _, f := range files {
		resp.Files = append(resp.Files, commitFile{Filename: f.Path, Patch: f.Patch})
	}
	writeJSON(w, http.StatusOK, resp)
}

type compareFile struct {
	Filename string `json:"filename"`
	Status   string `json:"status"`
}

type compareResponse struct {
	Files []compareFile `json:"files"`
}

func (s *Server) handleCompare(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	i := strings.Index(r.URL.Path, "/compare/")
	if i < 0 {
		http.NotFound(w, r)
		return
	}
	spec := strings.Trim(r.URL.Path[i+len("/compare/"):], "/")
	parts := strings.SplitN(spec, "...", 2)
	if len(parts) != 2 || parts[0] == "" || parts[1] == "" {
		http.Error(w, "bad compare spec", http.StatusBadRequest)
		return
	}
	changes, err := s.Store.Compare(parts[0], parts[1])
	if err != nil {
		serverError(w, "compare", err)
		return
	}
	resp := compareResponse{Files: make([]compareFile, 0, len(changes))}
	for _, c := range changes {
		resp.Files = append(resp.Files, compareFile{Filename: c.Path, Status: c.Status})
	}
	writeJSON(w, http.StatusOK, resp)
}

func (s *Server) handleContents(w http.ResponseWriter, r *http.Request) {
	path := contentsPath(r.URL.Path)
	switch r.Method {
	case http.MethodGet:
		if ref := r.URL.Query().Get("ref"); ref != "" {
			if s.Store.IsDir(path) {
				s.get(w, path)
				return
			}
			s.getAtRef(w, path, ref)
			return
		}
		s.get(w, path)
	case http.MethodPut:
		s.put(w, r, path)
	case http.MethodDelete:
		s.delete(w, r, path)
	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

func (s *Server) getAtRef(w http.ResponseWriter, path, ref string) {
	content, sha, err := s.Store.ReadAtRef(path, ref)
	if err == gitstore.ErrNotFound {
		http.Error(w, "not found", http.StatusNotFound)
		return
	}
	if err != nil {
		serverError(w, "read at ref", err)
		return
	}
	name := path
	if i := strings.LastIndex(path, "/"); i >= 0 {
		name = path[i+1:]
	}
	writeJSON(w, http.StatusOK, fileResponse{
		Name:    name,
		Path:    path,
		Type:    "file",
		SHA:     sha,
		Content: base64.StdEncoding.EncodeToString([]byte(content)),
	})
}

func (s *Server) get(w http.ResponseWriter, path string) {
	if s.Store.IsDir(path) {
		entries, lerr := s.Store.List(path)
		if lerr != nil {
			serverError(w, "list", lerr)
			return
		}
		list := make([]fileResponse, 0, len(entries))
		for _, e := range entries {
			list = append(list, fileResponse{Name: e.Name, Path: e.Path, Type: "file", SHA: e.SHA})
		}
		writeJSON(w, http.StatusOK, list)
		return
	}
	content, sha, err := s.Store.Read(path)
	if err == gitstore.ErrNotFound {
		http.Error(w, "not found", http.StatusNotFound)
		return
	}
	if err != nil {
		serverError(w, "read", err)
		return
	}
	name := path
	if i := strings.LastIndex(path, "/"); i >= 0 {
		name = path[i+1:]
	}
	writeJSON(w, http.StatusOK, fileResponse{
		Name:    name,
		Path:    path,
		Type:    "file",
		SHA:     sha,
		Content: base64.StdEncoding.EncodeToString([]byte(content)),
	})
}

func (s *Server) put(w http.ResponseWriter, r *http.Request, path string) {
	r.Body = http.MaxBytesReader(w, r.Body, MaxRequestBytes)
	var req putRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		clientError(w, "decode", err, http.StatusBadRequest)
		return
	}
	decoded, err := base64.StdEncoding.DecodeString(strings.ReplaceAll(req.Content, "\n", ""))
	if err != nil {
		clientError(w, "base64", err, http.StatusBadRequest)
		return
	}
	msg := req.Message
	if msg == "" {
		msg = "update " + path
	}
	newSHA, conflict, err := s.Store.Write(path, string(decoded), req.SHA, msg)
	if conflict {
		http.Error(w, "sha mismatch", http.StatusConflict)
		return
	}
	if err != nil {
		serverError(w, "write", err)
		return
	}
	var resp putResponse
	resp.Content.SHA = newSHA
	writeJSON(w, http.StatusOK, resp)
}

func (s *Server) delete(w http.ResponseWriter, r *http.Request, path string) {
	r.Body = http.MaxBytesReader(w, r.Body, MaxRequestBytes)
	var req deleteRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		clientError(w, "decode", err, http.StatusBadRequest)
		return
	}
	msg := req.Message
	if msg == "" {
		msg = "delete " + path
	}
	conflict, err := s.Store.Delete(path, req.SHA, msg)
	if conflict {
		http.Error(w, "sha mismatch", http.StatusConflict)
		return
	}
	if err == gitstore.ErrNotFound {
		http.Error(w, "not found", http.StatusNotFound)
		return
	}
	if err != nil {
		serverError(w, "delete", err)
		return
	}
	w.WriteHeader(http.StatusOK)
}

func (s *Server) handleRemote(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodGet:
		url, err := s.Store.RemoteURL()
		if err != nil {
			serverError(w, "remote get", err)
			return
		}
		writeJSON(w, http.StatusOK, map[string]string{"url": url})
	case http.MethodPost:
		r.Body = http.MaxBytesReader(w, r.Body, MaxRequestBytes)
		var req struct {
			URL string `json:"url"`
		}
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			clientError(w, "decode", err, http.StatusBadRequest)
			return
		}
		if req.URL == "" {
			http.Error(w, "empty url", http.StatusBadRequest)
			return
		}
		if err := s.Store.Reclone(req.URL); err != nil {
			serverError(w, "reclone", err)
			return
		}
		writeJSON(w, http.StatusOK, map[string]string{"url": req.URL})
	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	json.NewEncoder(w).Encode(v)
}
