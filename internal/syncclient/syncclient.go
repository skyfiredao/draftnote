package syncclient

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
)

var ErrConflict = errors.New("sha mismatch (409)")

type Client struct {
	BaseURL  string
	Owner    string
	Repo     string
	Username string
	Token    string
	UseAPI   bool
	Branch   string
	HTTP     *http.Client
}

type Entry struct {
	Name string `json:"name"`
	Path string `json:"path"`
	Type string `json:"type"`
	SHA  string `json:"sha"`
}

type File struct {
	Content string `json:"content"`
	SHA     string `json:"sha"`
}

type putBody struct {
	Message string `json:"message"`
	Content string `json:"content"`
	SHA     string `json:"sha,omitempty"`
	Branch  string `json:"branch,omitempty"`
}

type putResponse struct {
	Content struct {
		SHA string `json:"sha"`
	} `json:"content"`
}

func New(baseURL, owner, repo, username, token string, useAPI bool) *Client {
	return &Client{
		BaseURL:  strings.TrimRight(baseURL, "/"),
		Owner:    owner,
		Repo:     repo,
		Username: username,
		Token:    token,
		UseAPI:   useAPI,
		Branch:   "draftnote",
		HTTP:     http.DefaultClient,
	}
}

func (c *Client) contentsURL(path string) string {
	return fmt.Sprintf("%s/repos/%s/%s/contents/%s", c.BaseURL, c.Owner, c.Repo, path)
}

func (c *Client) contentsURLRef(path string) string {
	return c.contentsURL(path) + "?ref=" + url.QueryEscape(c.Branch)
}

func (c *Client) commitsURL(ref string) string {
	return fmt.Sprintf("%s/repos/%s/%s/commits/%s", c.BaseURL, c.Owner, c.Repo, ref)
}

func (c *Client) compareURL(base, head string) string {
	return fmt.Sprintf("%s/repos/%s/%s/compare/%s...%s", c.BaseURL, c.Owner, c.Repo, base, head)
}

type FileChange struct {
	Path   string
	Status string
}

var ErrCompareUnknownRef = errors.New("compare: unknown ref")

func (c *Client) Compare(base, head string) ([]FileChange, error) {
	if base == "" || head == "" {
		return nil, nil
	}
	if base == head {
		return nil, nil
	}
	req, err := http.NewRequest(http.MethodGet, c.compareURL(base, head), nil)
	if err != nil {
		return nil, err
	}
	resp, body, err := c.do(req)
	if err != nil {
		return nil, err
	}
	if resp.StatusCode == http.StatusNotFound {
		return nil, ErrCompareUnknownRef
	}
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("compare %s...%s: status %d: %s", base, head, resp.StatusCode, body)
	}
	var raw struct {
		Files []struct {
			Filename string `json:"filename"`
			Status   string `json:"status"`
		} `json:"files"`
	}
	if err := json.Unmarshal(body, &raw); err != nil {
		return nil, err
	}
	out := make([]FileChange, 0, len(raw.Files))
	for _, f := range raw.Files {
		status := f.Status
		switch status {
		case "added", "modified", "removed":
		case "renamed":
			status = "modified"
		default:
			if status == "" {
				status = "modified"
			}
		}
		out = append(out, FileChange{Path: f.Filename, Status: status})
	}
	return out, nil
}

type commitRef struct {
	SHA string `json:"sha"`
}

func (c *Client) Head(ref string) (string, error) {
	if ref == "" {
		ref = c.Branch
	}
	req, err := http.NewRequest(http.MethodGet, c.commitsURL(ref), nil)
	if err != nil {
		return "", err
	}
	resp, body, err := c.do(req)
	if err != nil {
		return "", err
	}
	if resp.StatusCode == http.StatusNotFound || resp.StatusCode == http.StatusUnprocessableEntity {
		return "", nil
	}
	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("head %s: status %d: %s", ref, resp.StatusCode, body)
	}
	var r commitRef
	if err := json.Unmarshal(body, &r); err != nil {
		return "", err
	}
	return r.SHA, nil
}

func (c *Client) do(req *http.Request) (*http.Response, []byte, error) {
	if c.UseAPI {
		req.Header.Set("Authorization", "token "+c.Token)
	} else {
		creds := c.Username + ":" + c.Token
		req.Header.Set("Authorization", "Basic "+base64.StdEncoding.EncodeToString([]byte(creds)))
	}
	req.Header.Set("Accept", "application/vnd.github+json")
	resp, err := c.HTTP.Do(req)
	if err != nil {
		return nil, nil, err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, nil, err
	}
	return resp, body, nil
}

func (c *Client) List(dir string) ([]Entry, error) {
	req, err := http.NewRequest(http.MethodGet, c.contentsURLRef(dir), nil)
	if err != nil {
		return nil, err
	}
	resp, body, err := c.do(req)
	if err != nil {
		return nil, err
	}
	if resp.StatusCode == http.StatusNotFound {
		return nil, nil
	}
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("list %s: status %d: %s", dir, resp.StatusCode, body)
	}
	var entries []Entry
	if err := json.Unmarshal(body, &entries); err != nil {
		return nil, err
	}
	return entries, nil
}

func (c *Client) Get(path string) (content string, sha string, err error) {
	req, err := http.NewRequest(http.MethodGet, c.contentsURLRef(path), nil)
	if err != nil {
		return "", "", err
	}
	resp, body, err := c.do(req)
	if err != nil {
		return "", "", err
	}
	if resp.StatusCode == http.StatusNotFound {
		return "", "", nil
	}
	if resp.StatusCode != http.StatusOK {
		return "", "", fmt.Errorf("get %s: status %d: %s", path, resp.StatusCode, body)
	}
	var f File
	if err := json.Unmarshal(body, &f); err != nil {
		return "", "", err
	}
	decoded, err := base64.StdEncoding.DecodeString(strings.ReplaceAll(f.Content, "\n", ""))
	if err != nil {
		return "", "", err
	}
	return string(decoded), f.SHA, nil
}

func (c *Client) Put(path, content, sha, message string) (newSHA string, err error) {
	pb := putBody{
		Message: message,
		Content: base64.StdEncoding.EncodeToString([]byte(content)),
		SHA:     sha,
		Branch:  c.Branch,
	}
	buf, err := json.Marshal(pb)
	if err != nil {
		return "", err
	}
	req, err := http.NewRequest(http.MethodPut, c.contentsURL(path), bytes.NewReader(buf))
	if err != nil {
		return "", err
	}
	req.Header.Set("Content-Type", "application/json")
	resp, body, err := c.do(req)
	if err != nil {
		return "", err
	}
	if resp.StatusCode == http.StatusConflict {
		return "", ErrConflict
	}
	if resp.StatusCode != http.StatusOK && resp.StatusCode != http.StatusCreated {
		return "", fmt.Errorf("put %s: status %d: %s", path, resp.StatusCode, body)
	}
	var pr putResponse
	if err := json.Unmarshal(body, &pr); err != nil {
		return "", err
	}
	return pr.Content.SHA, nil
}

type deleteBody struct {
	Message string `json:"message"`
	SHA     string `json:"sha"`
	Branch  string `json:"branch,omitempty"`
}

func (c *Client) Delete(path, sha, message string) error {
	db := deleteBody{Message: message, SHA: sha, Branch: c.Branch}
	buf, err := json.Marshal(db)
	if err != nil {
		return err
	}
	req, err := http.NewRequest(http.MethodDelete, c.contentsURL(path), bytes.NewReader(buf))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	resp, body, err := c.do(req)
	if err != nil {
		return err
	}
	if resp.StatusCode == http.StatusNotFound {
		return nil
	}
	if resp.StatusCode == http.StatusConflict {
		return ErrConflict
	}
	if resp.StatusCode != http.StatusOK && resp.StatusCode != http.StatusNoContent {
		return fmt.Errorf("delete %s: status %d: %s", path, resp.StatusCode, body)
	}
	return nil
}

func (c *Client) refsURL() string {
	return fmt.Sprintf("%s/repos/%s/%s/git/refs", c.BaseURL, c.Owner, c.Repo)
}

func (c *Client) EnsureBranch() error {
	if !c.UseAPI {
		return nil
	}
	sha, err := c.Head(c.Branch)
	if err != nil {
		return err
	}
	if sha != "" {
		return nil
	}
	treeSHA, err := c.createInitTree()
	if errors.Is(err, errBranchAPIUnsupported) {
		return nil
	}
	if err != nil {
		return err
	}
	commitSHA, err := c.createOrphanCommit(treeSHA, "init draftnote")
	if err != nil {
		return err
	}
	return c.createBranchRef(c.Branch, commitSHA)
}

var errBranchAPIUnsupported = errors.New("git data api unsupported")

func (c *Client) treesURL() string {
	return fmt.Sprintf("%s/repos/%s/%s/git/trees", c.BaseURL, c.Owner, c.Repo)
}

func (c *Client) gitCommitsURL() string {
	return fmt.Sprintf("%s/repos/%s/%s/git/commits", c.BaseURL, c.Owner, c.Repo)
}

func (c *Client) createInitTree() (string, error) {
	type treeEntry struct {
		Path    string `json:"path"`
		Mode    string `json:"mode"`
		Type    string `json:"type"`
		Content string `json:"content"`
	}
	payload := struct {
		Tree []treeEntry `json:"tree"`
	}{Tree: []treeEntry{{
		Path:    "README.md",
		Mode:    "100644",
		Type:    "blob",
		Content: "# draftnote\n",
	}}}
	buf, err := json.Marshal(payload)
	if err != nil {
		return "", err
	}
	req, err := http.NewRequest(http.MethodPost, c.treesURL(), bytes.NewReader(buf))
	if err != nil {
		return "", err
	}
	req.Header.Set("Content-Type", "application/json")
	resp, body, err := c.do(req)
	if err != nil {
		return "", err
	}
	if resp.StatusCode == http.StatusNotFound {
		return "", errBranchAPIUnsupported
	}
	if resp.StatusCode != http.StatusCreated && resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("create tree: status %d: %s", resp.StatusCode, body)
	}
	var out struct {
		SHA string `json:"sha"`
	}
	if err := json.Unmarshal(body, &out); err != nil {
		return "", err
	}
	return out.SHA, nil
}

func (c *Client) createOrphanCommit(treeSHA, message string) (string, error) {
	payload := struct {
		Message string   `json:"message"`
		Tree    string   `json:"tree"`
		Parents []string `json:"parents"`
	}{Message: message, Tree: treeSHA, Parents: []string{}}
	buf, err := json.Marshal(payload)
	if err != nil {
		return "", err
	}
	req, err := http.NewRequest(http.MethodPost, c.gitCommitsURL(), bytes.NewReader(buf))
	if err != nil {
		return "", err
	}
	req.Header.Set("Content-Type", "application/json")
	resp, body, err := c.do(req)
	if err != nil {
		return "", err
	}
	if resp.StatusCode != http.StatusCreated && resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("create commit: status %d: %s", resp.StatusCode, body)
	}
	var out struct {
		SHA string `json:"sha"`
	}
	if err := json.Unmarshal(body, &out); err != nil {
		return "", err
	}
	return out.SHA, nil
}

func (c *Client) createBranchRef(branch, sha string) error {
	payload := struct {
		Ref string `json:"ref"`
		SHA string `json:"sha"`
	}{
		Ref: "refs/heads/" + branch,
		SHA: sha,
	}
	buf, err := json.Marshal(payload)
	if err != nil {
		return err
	}
	req, err := http.NewRequest(http.MethodPost, c.refsURL(), bytes.NewReader(buf))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	resp, body, err := c.do(req)
	if err != nil {
		return err
	}
	if resp.StatusCode == http.StatusCreated || resp.StatusCode == http.StatusOK {
		return nil
	}
	if resp.StatusCode == http.StatusUnprocessableEntity {
		return nil
	}
	return fmt.Errorf("create ref %s: status %d: %s", branch, resp.StatusCode, body)
}

func (c *Client) GetRemote() (string, error) {
	req, err := http.NewRequest(http.MethodGet, c.BaseURL+"/remote", nil)
	if err != nil {
		return "", err
	}
	resp, body, err := c.do(req)
	if err != nil {
		return "", err
	}
	if resp.StatusCode == http.StatusNotFound {
		return "", nil
	}
	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("get remote: status %d: %s", resp.StatusCode, body)
	}
	var out struct {
		URL string `json:"url"`
	}
	if err := json.Unmarshal(body, &out); err != nil {
		return "", err
	}
	return out.URL, nil
}

func (c *Client) PostReclone(newURL string) error {
	buf, err := json.Marshal(struct {
		URL string `json:"url"`
	}{URL: newURL})
	if err != nil {
		return err
	}
	req, err := http.NewRequest(http.MethodPost, c.BaseURL+"/remote", bytes.NewReader(buf))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	resp, body, err := c.do(req)
	if err != nil {
		return err
	}
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("reclone: status %d: %s", resp.StatusCode, body)
	}
	return nil
}

type CommitInfo struct {
	SHA        string `json:"sha"`
	AuthorName string `json:"author_name"`
	Date       string `json:"date"`
	Subject    string `json:"subject"`
}

func (c *Client) FileHistory(path string, page, perPage int) ([]CommitInfo, error) {
	u := fmt.Sprintf("%s/repos/%s/%s/commits?path=%s&sha=%s&page=%d&per_page=%d",
		c.BaseURL, c.Owner, c.Repo, url.QueryEscape(path), c.Branch, page, perPage)
	req, err := http.NewRequest(http.MethodGet, u, nil)
	if err != nil {
		return nil, err
	}
	resp, body, err := c.do(req)
	if err != nil {
		return nil, err
	}
	if resp.StatusCode == http.StatusNotFound {
		return nil, nil
	}
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("history %s: status %d: %s", path, resp.StatusCode, body)
	}
	var raw []struct {
		SHA    string `json:"sha"`
		Commit struct {
			Message string `json:"message"`
			Author  struct {
				Name string `json:"name"`
				Date string `json:"date"`
			} `json:"author"`
		} `json:"commit"`
	}
	if err := json.Unmarshal(body, &raw); err != nil {
		return nil, err
	}
	out := make([]CommitInfo, 0, len(raw))
	for _, r := range raw {
		out = append(out, CommitInfo{
			SHA:        r.SHA,
			AuthorName: r.Commit.Author.Name,
			Date:       r.Commit.Author.Date,
			Subject:    r.Commit.Message,
		})
	}
	return out, nil
}

type commitFilePatch struct {
	Filename string `json:"filename"`
	Patch    string `json:"patch"`
}

func (c *Client) CommitPatch(sha, path string) (string, error) {
	req, err := http.NewRequest(http.MethodGet, c.commitsURL(sha), nil)
	if err != nil {
		return "", err
	}
	resp, body, err := c.do(req)
	if err != nil {
		return "", err
	}
	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("commit %s: status %d: %s", sha, resp.StatusCode, body)
	}
	var detail struct {
		Files []commitFilePatch `json:"files"`
	}
	if err := json.Unmarshal(body, &detail); err != nil {
		return "", err
	}
	for _, f := range detail.Files {
		if f.Filename == path {
			return f.Patch, nil
		}
	}
	return "", nil
}

func (c *Client) GetAtRef(path, ref string) (content string, sha string, err error) {
	u := c.contentsURL(path) + "?ref=" + url.QueryEscape(ref)
	req, err := http.NewRequest(http.MethodGet, u, nil)
	if err != nil {
		return "", "", err
	}
	resp, body, err := c.do(req)
	if err != nil {
		return "", "", err
	}
	if resp.StatusCode == http.StatusNotFound {
		return "", "", nil
	}
	if resp.StatusCode != http.StatusOK {
		return "", "", fmt.Errorf("get at ref %s: status %d: %s", path, resp.StatusCode, body)
	}
	var f File
	if err := json.Unmarshal(body, &f); err != nil {
		return "", "", err
	}
	decoded, err := base64.StdEncoding.DecodeString(strings.ReplaceAll(f.Content, "\n", ""))
	if err != nil {
		return "", "", err
	}
	return string(decoded), f.SHA, nil
}
