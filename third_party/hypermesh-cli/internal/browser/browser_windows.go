package browser

import (
	"golang.org/x/sys/windows"
	"runtime"
	"syscall"
)

func openNative(target string) error {
	file, err := windows.UTF16PtrFromString(target)
	if err != nil {
		return errBrowser
	}
	verb, _ := windows.UTF16PtrFromString("open")
	result := make(chan error, 1)
	go func() {
		runtime.LockOSThread()
		defer runtime.UnlockOSThread()
		// Shell extensions may require an STA. Keep initialization and launch on
		// this thread, and balance both S_OK and S_FALSE with CoUninitialize.
		err := windows.CoInitializeEx(0, 2|4)
		if err != nil && err != syscall.Errno(1) {
			result <- errBrowser
			return
		}
		defer windows.CoUninitialize()
		result <- windows.ShellExecute(0, verb, file, nil, nil, windows.SW_SHOWNORMAL)
	}()
	return <-result
}
