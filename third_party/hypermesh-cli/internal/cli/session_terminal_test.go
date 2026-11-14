package cli

import (
	"bufio"
	"bytes"
	"errors"
	"reflect"
	"strings"
	"testing"
)

func TestRawTerminalInputPreservesLongLinesAndCancelsBeforeSending(t *testing.T) {
	for _, count := range []int{5000, 16385} {
		var echo, output bytes.Buffer
		input := &terminalInput{input: bufio.NewReader(strings.NewReader(strings.Repeat("x", count) + "\n/quit\n")), echo: &echo}
		var sent []string
		err := sessionTerminal(input, &output, terminalTestSession, func(text string) error { sent = append(sent, text); return nil })
		if count == 5000 && (err != nil || len(sent) != 1 || len(sent[0]) != count) {
			t.Fatalf("long line: err=%v sent=%v", err, len(sent))
		}
		if count > 16384 && (err == nil || len(sent) != 0) {
			t.Fatalf("oversized line: err=%v sent=%v", err, len(sent))
		}
	}
	for _, control := range []byte{3, 4} {
		input := &terminalInput{input: bufio.NewReader(strings.NewReader("unsent" + string(control))), echo: &bytes.Buffer{}}
		if err := sessionTerminal(input, &bytes.Buffer{}, terminalTestSession, func(string) error { t.Fatal("cancelled input sent"); return nil }); err != nil {
			t.Fatal(err)
		}
	}
	input := &terminalInput{input: bufio.NewReader(strings.NewReader("aé\x7fb\n/quit\n")), echo: &bytes.Buffer{}}
	if err := sessionTerminal(input, &bytes.Buffer{}, terminalTestSession, func(text string) error {
		if text != "ab" {
			t.Fatalf("edited input %q", text)
		}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
}

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
