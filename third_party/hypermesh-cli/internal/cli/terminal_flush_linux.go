package cli

import "golang.org/x/sys/unix"

func flushTerminal(fd int) error { return unix.IoctlSetInt(fd, unix.TCFLSH, unix.TCIFLUSH) }
