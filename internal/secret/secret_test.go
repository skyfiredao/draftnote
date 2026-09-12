package secret

import (
	"encoding/base64"
	"strings"
	"testing"
)

func TestSealOpenRoundTrip(t *testing.T) {
	cases := []string{
		"a",
		"ghp_abc123XYZ_token",
		"multi word token with spaces",
		strings.Repeat("x", 512),
		"中文 token 也能过",
	}
	for _, in := range cases {
		blob, err := Seal(in)
		if err != nil {
			t.Fatalf("seal(%q): %v", in, err)
		}
		got, err := Open(blob)
		if err != nil {
			t.Fatalf("open(%q): %v", in, err)
		}
		if got != in {
			t.Errorf("round trip %q -> %q", in, got)
		}
	}
}

func TestSealEmptyReturnsEmpty(t *testing.T) {
	blob, err := Seal("")
	if err != nil {
		t.Fatal(err)
	}
	if blob != "" {
		t.Errorf("expected empty for empty input, got %q", blob)
	}
	got, err := Open("")
	if err != nil {
		t.Fatal(err)
	}
	if got != "" {
		t.Errorf("expected empty from empty blob, got %q", got)
	}
}

func TestSealProducesDifferentCiphertextEachCall(t *testing.T) {
	a, _ := Seal("same-input")
	b, _ := Seal("same-input")
	if a == b {
		t.Error("two seals of the same plaintext should differ (fresh nonce)")
	}
}

func TestOpenRejectsTampered(t *testing.T) {
	blob, err := Seal("secret token")
	if err != nil {
		t.Fatal(err)
	}
	raw, _ := base64.StdEncoding.DecodeString(blob)

	raw[len(raw)-1] ^= 0x01
	tampered := base64.StdEncoding.EncodeToString(raw)
	if _, err := Open(tampered); err == nil {
		t.Error("tampered blob should not decrypt")
	}
}

func TestOpenRejectsUnknownVersion(t *testing.T) {
	blob, _ := Seal("x")
	raw, _ := base64.StdEncoding.DecodeString(blob)
	raw[0] = 0xff
	bad := base64.StdEncoding.EncodeToString(raw)
	if _, err := Open(bad); err == nil {
		t.Error("wrong version byte should be rejected")
	}
}

func TestOpenRejectsGarbage(t *testing.T) {
	if _, err := Open("not-base64!!!"); err == nil {
		t.Error("garbage input should fail")
	}
	if _, err := Open("AA=="); err == nil {
		t.Error("blob shorter than nonce should fail")
	}
}
