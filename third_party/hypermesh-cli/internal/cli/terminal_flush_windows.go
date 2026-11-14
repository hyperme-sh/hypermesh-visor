package cli

import "golang.org/x/sys/windows"

func flushTerminal(fd int) error {
	ok, _, err := windows.NewLazySystemDLL("kernel32.dll").NewProc("FlushConsoleInputBuffer").Call(uintptr(fd))
	if ok == 0 {
		return err
	}
	return nil
}
