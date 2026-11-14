package cli

import (
	"bufio"
	"bytes"
	"fmt"
	"io"
	"unicode/utf8"
)

// Own the editable line while the OS terminal is raw, avoiding kernel line truncation.
type terminalInput struct {
	input   *bufio.Reader
	echo    io.Writer
	pending []byte
}

func (r *terminalInput) Read(output []byte) (int, error) {
	if len(output) == 0 {
		return 0, nil
	}
	if len(r.pending) == 0 {
		var line []byte
		for {
			b, err := r.input.ReadByte()
			if err != nil {
				return 0, err
			}
			switch b {
			case 3, 4:
				return 0, io.EOF
			case 8, 127:
				if len(line) > 0 {
					_, width := utf8.DecodeLastRune(line)
					line = line[:len(line)-width]
					if _, err := io.WriteString(r.echo, "\b \b"); err != nil {
						return 0, err
					}
				}
			case '\r', '\n':
				if len(line)+1 > 16384 {
					return 0, fmt.Errorf("prompt exceeds 16 KiB; nothing was sent")
				}
				r.pending = append(line, '\n')
				if _, err := io.WriteString(r.echo, "\n"); err != nil {
					return 0, err
				}
			default:
				if b < 32 && b != '\t' {
					return 0, fmt.Errorf("unsupported terminal control input; nothing was sent")
				}
				line = append(line, b)
				if len(line) > 16384 {
					return 0, fmt.Errorf("prompt exceeds 16 KiB; nothing was sent")
				}
				if _, err := r.echo.Write([]byte{b}); err != nil {
					return 0, err
				}
			}
			if len(r.pending) != 0 {
				break
			}
		}
	}
	count := copy(output, r.pending)
	r.pending = r.pending[count:]
	return count, nil
}

type terminalOutput struct{ io.Writer }

func (w terminalOutput) Write(value []byte) (int, error) {
	_, err := w.Writer.Write(bytes.ReplaceAll(value, []byte("\n"), []byte("\r\n")))
	if err != nil {
		return 0, err
	}
	return len(value), nil
}
