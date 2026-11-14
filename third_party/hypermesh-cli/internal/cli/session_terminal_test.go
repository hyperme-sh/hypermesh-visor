package cli

import (
	"bytes"
	"errors"
	"reflect"
	"strings"
	"testing"
)

const terminalTestSession = "00000000-0000-4000-8000-000000000001"

func TestSessionTerminalReadsEachLineOnceAndDoesNotRetry(t *testing.T) {
	var output bytes.Buffer
	var sent []string
	err := sessionTerminal(strings.NewReader("\nfirst\r\nsecond\n/quit\nnot sent\n"), &output, terminalTestSession, func(text string) error {
		sent = append(sent, text)
		return errors.New("unavailable; not retried")
	})
	if err != nil || !reflect.DeepEqual(sent, []string{"first", "second"}) || strings.Count(output.String(), "not retried") != 2 {
		t.Fatalf("err=%v sent=%q output=%q", err, sent, output.String())
	}
}

func TestSessionTerminalBoundsInputBeforeSending(t *testing.T) {
	for _, suffix := range []string{"", "\nnext\n"} {
		sent := 0
		err := sessionTerminal(strings.NewReader(strings.Repeat("x", 16385)+suffix), &bytes.Buffer{}, terminalTestSession, func(string) error { sent++; return nil })
		if err == nil || sent != 0 {
			t.Fatalf("oversized input: err=%v sent=%d", err, sent)
		}
	}
	var sent []string
	err := sessionTerminal(strings.NewReader("last without newline"), &bytes.Buffer{}, terminalTestSession, func(text string) error { sent = append(sent, text); return nil })
	if err != nil || !reflect.DeepEqual(sent, []string{"last without newline"}) {
		t.Fatalf("EOF: err=%v sent=%q", err, sent)
	}
}

func TestSessionTerminalRejectsInvalidSessionBeforeReading(t *testing.T) {
	for _, session := range []string{"", "../other", "--option", terminalTestSession + "/extra"} {
		var output bytes.Buffer
		err := sessionTerminal(strings.NewReader("secret\n"), &output, session, func(string) error { t.Fatal("invalid session sent a prompt"); return nil })
		if err == nil || output.Len() != 0 {
			t.Fatalf("accepted %q", session)
		}
	}
}
