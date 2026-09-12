package conflict

import "strings"

const (
	markerLocal  = "<<<<<<< local version"
	markerSep    = "======="
	markerRemote = ">>>>>>> remote version"
)

func Merge(local, remote string) string {
	if local == remote {
		return local
	}
	localLines := splitLines(local)
	remoteLines := splitLines(remote)
	hunks := diff(localLines, remoteLines)

	var out []string
	for _, h := range hunks {
		switch {
		case h.equal:
			out = append(out, h.local...)
		case len(h.local) == 0:
			out = append(out, h.remote...)
		case len(h.remote) == 0:
			out = append(out, h.local...)
		default:
			out = append(out, markerLocal)
			out = append(out, h.local...)
			out = append(out, markerSep)
			out = append(out, h.remote...)
			out = append(out, markerRemote)
		}
	}
	return joinLines(out, local, remote)
}

func HasMarkers(text string) bool {
	for _, line := range strings.Split(text, "\n") {
		if line == markerLocal || line == markerSep || line == markerRemote {
			return true
		}
	}
	return false
}

type hunk struct {
	equal  bool
	local  []string
	remote []string
}

func diff(a, b []string) []hunk {
	lcs := lcsMatrix(a, b)
	var hunks []hunk
	i, j := 0, 0
	var pendingA, pendingB []string
	flush := func() {
		if len(pendingA) > 0 || len(pendingB) > 0 {
			hunks = append(hunks, hunk{local: pendingA, remote: pendingB})
			pendingA, pendingB = nil, nil
		}
	}
	for i < len(a) && j < len(b) {
		if a[i] == b[j] {
			flush()
			hunks = append(hunks, hunk{equal: true, local: []string{a[i]}})
			i++
			j++
		} else if lcs[i+1][j] >= lcs[i][j+1] {
			pendingA = append(pendingA, a[i])
			i++
		} else {
			pendingB = append(pendingB, b[j])
			j++
		}
	}
	for i < len(a) {
		pendingA = append(pendingA, a[i])
		i++
	}
	for j < len(b) {
		pendingB = append(pendingB, b[j])
		j++
	}
	flush()
	return mergeAdjacent(hunks)
}

func mergeAdjacent(hunks []hunk) []hunk {
	var out []hunk
	for _, h := range hunks {
		if h.equal {
			if len(out) > 0 && out[len(out)-1].equal {
				out[len(out)-1].local = append(out[len(out)-1].local, h.local...)
				continue
			}
		}
		out = append(out, h)
	}
	return out
}

func lcsMatrix(a, b []string) [][]int {
	m, n := len(a), len(b)
	lcs := make([][]int, m+1)
	for i := range lcs {
		lcs[i] = make([]int, n+1)
	}
	for i := m - 1; i >= 0; i-- {
		for j := n - 1; j >= 0; j-- {
			if a[i] == b[j] {
				lcs[i][j] = lcs[i+1][j+1] + 1
			} else if lcs[i+1][j] >= lcs[i][j+1] {
				lcs[i][j] = lcs[i+1][j]
			} else {
				lcs[i][j] = lcs[i][j+1]
			}
		}
	}
	return lcs
}

func splitLines(s string) []string {
	if s == "" {
		return nil
	}
	return strings.Split(strings.TrimSuffix(s, "\n"), "\n")
}

func joinLines(lines []string, local, remote string) string {
	joined := strings.Join(lines, "\n")
	if strings.HasSuffix(local, "\n") || strings.HasSuffix(remote, "\n") {
		joined += "\n"
	}
	return joined
}
