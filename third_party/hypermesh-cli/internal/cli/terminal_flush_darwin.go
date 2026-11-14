package cli

import "golang.org/x/sys/unix"

func flushTerminal(fd int) error { return unix.IoctlSetPointerInt(fd, unix.TIOCFLUSH, 1) }
