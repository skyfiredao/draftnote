package secret

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"encoding/base64"
	"errors"
	"io"
)

const version byte = 0x01

var appKey = [32]byte{
	0x55, 0x6c, 0x0f, 0x83, 0x91, 0x6c, 0x75, 0x89, 0x3d, 0xaf, 0x41, 0x40,
	0x7d, 0xa6, 0x87, 0x35, 0x45, 0x3f, 0x51, 0xd2, 0xf0, 0x8c, 0x45, 0x83,
	0x49, 0xee, 0x5c, 0xd0, 0x92, 0x20, 0x84, 0x7c,
}

func Seal(plaintext string) (string, error) {
	if plaintext == "" {
		return "", nil
	}
	block, err := aes.NewCipher(appKey[:])
	if err != nil {
		return "", err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return "", err
	}
	nonce := make([]byte, gcm.NonceSize())
	if _, err := io.ReadFull(rand.Reader, nonce); err != nil {
		return "", err
	}
	ct := gcm.Seal(nil, nonce, []byte(plaintext), nil)
	buf := make([]byte, 0, 1+len(nonce)+len(ct))
	buf = append(buf, version)
	buf = append(buf, nonce...)
	buf = append(buf, ct...)
	return base64.StdEncoding.EncodeToString(buf), nil
}

func Open(blob string) (string, error) {
	if blob == "" {
		return "", nil
	}
	raw, err := base64.StdEncoding.DecodeString(blob)
	if err != nil {
		return "", err
	}
	if len(raw) < 1 {
		return "", errors.New("secret: blob too short")
	}
	if raw[0] != version {
		return "", errors.New("secret: unsupported version")
	}
	block, err := aes.NewCipher(appKey[:])
	if err != nil {
		return "", err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return "", err
	}
	if len(raw) < 1+gcm.NonceSize() {
		return "", errors.New("secret: blob too short")
	}
	nonce := raw[1 : 1+gcm.NonceSize()]
	ct := raw[1+gcm.NonceSize():]
	pt, err := gcm.Open(nil, nonce, ct, nil)
	if err != nil {
		return "", err
	}
	return string(pt), nil
}
