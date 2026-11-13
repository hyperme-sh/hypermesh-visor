// Package browser opens web destinations without a command interpreter.
package browser

import (
	"errors"
	"net/url"
	"os"
	"runtime"
	"strings"
	"unicode"
)

var errBrowser = errors.New("could not open a browser")

func webURL(raw string) (string, error) {
	if len(raw) == 0 || len(raw) > 16384 || strings.IndexFunc(raw, unicode.IsControl) >= 0 {
		return "", errBrowser
	}
	u, err := url.Parse(raw)
	if err != nil || u.Opaque != "" || u.Hostname() == "" || u.User != nil {
		return "", errBrowser
	}
	u.Scheme = strings.ToLower(u.Scheme)
	if u.Scheme != "https" && u.Scheme != "http" {
		return "", errBrowser
	}
	return u.String(), nil
}

func Open(raw string) error {
	return openWith(raw, openNative)
}
func openWith(raw string, launch func(string) error) error {
	target, err := webURL(raw)
	if err != nil {
		return errBrowser
	}
	if launch(target) != nil {
		return errBrowser
	}
	return nil
}

// Native desktop platforms try their registered handler. Unattended callers
// should select device login explicitly; a failed launch falls back there.
func DisplayAvailable() bool { return displayAvailable(runtime.GOOS, os.Getenv) }
func displayAvailable(platform string, env func(string) string) bool {
	switch platform {
	case "darwin", "windows":
		return true
	case "linux":
		return strings.TrimSpace(env("DISPLAY")) != "" || strings.TrimSpace(env("WAYLAND_DISPLAY")) != ""
	default:
		return false
	}
}
