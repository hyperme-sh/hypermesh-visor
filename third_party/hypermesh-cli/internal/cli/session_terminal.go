package cli

import (
	"bufio"
	"errors"
	"fmt"
	"io"
	"os"
	"regexp"
	"strings"

	"github.com/FyberLabs/hypermesh-cli/internal/api"
	"github.com/spf13/cobra"
	"golang.org/x/term"
)

var terminalSession = regexp.MustCompile(`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`)
var terminalInstance = regexp.MustCompile(`^[a-zA-Z0-9-]{1,64}$`)

func newSessionTerminalCmd(r *run) *cobra.Command {
	var session, instance string
	var externallyRaw bool
	cmd := &cobra.Command{
		Use:   "session-terminal --session UUID",
		Short: "Enter prompts for one existing local Visor session",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) (result error) {
			if !terminalSession.MatchString(session) {
				return fmt.Errorf("a canonical session UUID is required")
			}
			if instance != "" {
				if !terminalInstance.MatchString(instance) {
					return fmt.Errorf("invalid Visor instance")
				}
				prior, present := os.LookupEnv("HYPERMESH_VISOR_INSTANCE")
				if err := os.Setenv("HYPERMESH_VISOR_INSTANCE", instance); err != nil {
					return err
				}
				defer func() {
					if present {
						_ = os.Setenv("HYPERMESH_VISOR_INSTANCE", prior)
					} else {
						_ = os.Unsetenv("HYPERMESH_VISOR_INSTANCE")
					}
				}()
			}
			input, output := cmd.InOrStdin(), cmd.OutOrStdout()
			rawInput := externallyRaw
			if file, ok := input.(*os.File); ok && term.IsTerminal(int(file.Fd())) {
				fd := int(file.Fd())
				state, err := term.MakeRaw(fd)
				if err != nil {
					return err
				}
				defer func() { result = errors.Join(result, flushTerminal(fd), term.Restore(fd, state)) }()
				rawInput = true
			}
			if rawInput {
				output = terminalOutput{output}
				input = &terminalInput{input: bufio.NewReader(input), echo: output}
			}
			return sessionTerminal(input, output, strings.ToLower(session), func(text string) error {
				// Empty URL selects the private local transport, regardless of TCP environment settings.
				raw, err := r.client.PostVisorPrompt("", strings.ToLower(session), text, "")
				if err != nil {
					return fmt.Errorf("prompt failed; it was not retried")
				}
				return writeModelText(output, api.RedactKey(string(raw), r.cfg.APIKey))
			})
		},
	}
	cmd.Flags().StringVar(&session, "session", "", "existing session UUID")
	cmd.Flags().StringVar(&instance, "instance", "", "Windows local pipe instance")
	cmd.Flags().BoolVar(&externallyRaw, "raw-terminal", false, "caller owns raw terminal settings (WSL launcher)")
	return cmd
}

func sessionTerminal(input io.Reader, output io.Writer, session string, send func(string) error) error {
	if !terminalSession.MatchString(session) {
		return fmt.Errorf("a canonical session UUID is required")
	}
	if _, err := fmt.Fprintf(output, "Visor session %s\nEnter one prompt per line; /quit or EOF closes this terminal.\n", session); err != nil {
		return err
	}
	reader := bufio.NewReader(input)
	for {
		if _, err := fmt.Fprint(output, "> "); err != nil {
			return err
		}
		// Keep a byte bound before dispatch, including an unterminated final line.
		var line []byte
		var err error
		for len(line) <= 16384 {
			var next byte
			next, err = reader.ReadByte()
			if err != nil {
				break
			}
			line = append(line, next)
			if next == '\n' {
				break
			}
		}
		if len(line) > 16384 {
			return fmt.Errorf("prompt exceeds 16 KiB; nothing was sent")
		}
		if err != nil && err != io.EOF {
			return err
		}
		text := strings.TrimRight(string(line), "\r\n")
		if text == "/quit" {
			return nil
		}
		if strings.TrimSpace(text) != "" {
			if failure := send(text); failure != nil {
				if _, writeErr := fmt.Fprintln(output, failure); writeErr != nil {
					return writeErr
				}
			}
		}
		if err == io.EOF {
			return nil
		}
	}
}
