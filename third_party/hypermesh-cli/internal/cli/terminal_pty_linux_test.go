package cli

import (
	"bufio"
	"fmt"
	"io"
	"os"
	"reflect"
	"strings"
	"testing"
	"time"

	"golang.org/x/sys/unix"
	"golang.org/x/term"
)

func TestRawTerminalPTYDoesNotTruncatePastedPrompt(t *testing.T) {
	for _, count := range []int{5000, 16385} {
		t.Run(fmt.Sprint(count), func(t *testing.T) {
			fd, err := unix.Open("/dev/ptmx", unix.O_RDWR|unix.O_NOCTTY|unix.O_NONBLOCK, 0)
			if err != nil {
				t.Fatal(err)
			}
			master := os.NewFile(uintptr(fd), "pty-master")
			defer master.Close()
			if err := unix.IoctlSetPointerInt(fd, unix.TIOCSPTLCK, 0); err != nil {
				t.Fatal(err)
			}
			number, err := unix.IoctlGetInt(fd, unix.TIOCGPTN)
			if err != nil {
				t.Fatal(err)
			}
			slave, err := os.OpenFile(fmt.Sprintf("/dev/pts/%d", number), os.O_RDWR|unix.O_NOCTTY, 0)
			if err != nil {
				t.Fatal(err)
			}
			defer slave.Close()
			state, err := term.MakeRaw(int(slave.Fd()))
			if err != nil {
				t.Fatal(err)
			}
			defer term.Restore(int(slave.Fd()), state)
			type outcome struct {
				err  error
				sent []int
			}
			done := make(chan outcome, 1)
			go func() {
				var result outcome
				input := &terminalInput{input: bufio.NewReader(slave), echo: io.Discard}
				result.err = sessionTerminal(input, io.Discard, terminalTestSession, func(text string) error { result.sent = append(result.sent, len(text)); return nil })
				done <- result
			}()
			written := make(chan error, 1)
			go func() { _, err := io.WriteString(master, strings.Repeat("x", count)+"\n/quit\n"); written <- err }()
			select {
			case result := <-done:
				if count == 5000 && (result.err != nil || !reflect.DeepEqual(result.sent, []int{5000})) {
					t.Fatalf("truncated input: %+v", result)
				}
				if count > 16384 && (result.err == nil || len(result.sent) != 0) {
					t.Fatalf("oversized input sent: %+v", result)
				}
			case <-time.After(5 * time.Second):
				t.Fatal("PTY reader timed out")
			}
			if err := flushTerminal(int(slave.Fd())); err != nil {
				t.Fatal(err)
			}
			if err := term.Restore(int(slave.Fd()), state); err != nil {
				t.Fatal(err)
			}
			restored, err := term.GetState(int(slave.Fd()))
			if err != nil || !reflect.DeepEqual(state, restored) {
				t.Fatalf("terminal state not restored: %v", err)
			}
			select {
			case err := <-written:
				if err != nil {
					t.Fatal(err)
				}
			case <-time.After(time.Second):
				t.Fatal("PTY writer timed out")
			}
		})
	}
}
