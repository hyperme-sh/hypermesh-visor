//go:build !windows

package browser

import (
	"os/exec"
	"runtime"
)

func openNative(target string) error {
	switch runtime.GOOS {
	case "darwin":
		return exec.Command("/usr/bin/open", target).Run()
	case "linux":
		cmd := exec.Command("xdg-open", target)
		if err := cmd.Start(); err != nil {
			return err
		}
		go func() { _ = cmd.Wait() }()
		return nil
	default:
		return errBrowser
	}
}
