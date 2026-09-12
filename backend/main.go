package main

import (
	"flag"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"draftnote-backend/gitstore"
	"draftnote-backend/server"
)

func main() {
	dataFlag := flag.String("data", "", "working directory that holds the draftnote clone (env DRAFTNOTE_DATA, default /data)")
	addrFlag := flag.String("addr", "", "listen address like :8080 or 0.0.0.0:8080 (env DRAFTNOTE_ADDR)")
	portFlag := flag.String("port", "", "listen port like 8015 (env DRAFTNOTE_PORT); ignored when -addr is set")
	remoteFlag := flag.String("remote", "", "remote git URL to clone into <data>/draftnote when it does not exist (env DRAFTNOTE_REMOTE_URL)")
	flag.Parse()

	dataDir := firstNonEmpty(*dataFlag, os.Getenv("DRAFTNOTE_DATA"), "/data")
	addr := resolveAddr(*addrFlag, *portFlag)
	remoteURL := firstNonEmpty(*remoteFlag, os.Getenv("DRAFTNOTE_REMOTE_URL"))
	username := os.Getenv("DRAFTNOTE_USERNAME")
	token := os.Getenv("DRAFTNOTE_TOKEN")

	repoDir := filepath.Join(dataDir, "draftnote")

	store, err := gitstore.CloneOrOpen(repoDir, remoteURL)
	if err != nil {
		log.Fatalf("open store: %v", err)
	}
	srv := &server.Server{Store: store, Username: username, Token: token}

	log.Printf("draftnote backend listening on %s, repo=%s", addr, repoDir)
	if err := http.ListenAndServe(addr, srv.Handler()); err != nil {
		log.Fatal(err)
	}
}

func resolveAddr(flagAddr, flagPort string) string {
	if flagAddr != "" {
		return flagAddr
	}
	if v := os.Getenv("DRAFTNOTE_ADDR"); v != "" {
		return v
	}
	if flagPort != "" {
		return normalizePort(flagPort)
	}
	if v := os.Getenv("DRAFTNOTE_PORT"); v != "" {
		return normalizePort(v)
	}
	return ":8080"
}

func normalizePort(p string) string {
	p = strings.TrimSpace(p)
	if p == "" {
		return ":8080"
	}
	if strings.HasPrefix(p, ":") {
		return p
	}
	if _, err := strconv.Atoi(p); err == nil {
		return ":" + p
	}
	return p
}

func firstNonEmpty(vals ...string) string {
	for _, v := range vals {
		if v != "" {
			return v
		}
	}
	return ""
}
