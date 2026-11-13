package browser

import (
	"errors"
	"strings"
	"testing"
)

func TestWebDestinationsOnly(t *testing.T) {
	for _, raw := range []string{"", "file:///private", "javascript:alert(1)", "https:opaque", "https://user:secret@example.com/", "https:///missing", "https://example.com/\nprivate", "--argument", strings.Repeat("x", 16385)} {
		calls := 0
		err := openWith(raw, func(string) error { calls++; return nil })
		if err == nil || calls != 0 || strings.Contains(err.Error(), raw) && raw != "" {
			t.Fatalf("invalid target was launched or disclosed")
		}
	}
}
func TestURLIsOneDataValueAndErrorsArePrivate(t *testing.T) {
	raw := "https://example.com/auth?state=private&arg=%22%26calc%22#fragment"
	var got string
	if err := openWith(raw, func(s string) error { got = s; return nil }); err != nil || got != raw {
		t.Fatal("URL changed")
	}
	err := openWith(raw, func(string) error { return errors.New("private launcher details") })
	if err == nil || strings.Contains(err.Error(), "private") {
		t.Fatal("launcher error disclosed")
	}
}
func TestNativeDisplaySelection(t *testing.T) {
	empty := func(string) string { return "" }
	for _, platform := range []string{"windows", "darwin"} {
		if !displayAvailable(platform, empty) {
			t.Fatal("native handler was not selected")
		}
	}
	if displayAvailable("linux", empty) || displayAvailable("unknown", empty) {
		t.Fatal("invented desktop")
	}
	if !displayAvailable("linux", func(k string) string {
		if k == "WAYLAND_DISPLAY" {
			return "wayland-0"
		}
		return ""
	}) {
		t.Fatal("Wayland not detected")
	}
}
