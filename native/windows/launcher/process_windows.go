package main

import (
	"fmt"
	"runtime"
	"unicode/utf16"
	"unsafe"

	"golang.org/x/sys/windows"
)

func newJob() (windows.Handle, error) {
	job, err := windows.CreateJobObject(nil, nil)
	if err != nil {
		return 0, err
	}
	limits := windows.JOBOBJECT_EXTENDED_LIMIT_INFORMATION{}
	limits.BasicLimitInformation.LimitFlags = windows.JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
	_, err = windows.SetInformationJobObject(job, windows.JobObjectExtendedLimitInformation, uintptr(unsafe.Pointer(&limits)), uint32(unsafe.Sizeof(limits)))
	if err != nil {
		windows.CloseHandle(job)
		return 0, err
	}
	return job, nil
}

// Job assignment occurs during process creation, before any child code runs.
// The job handle is never inherited, so launcher death closes its last handle.
func spawn(job windows.Handle, executable string, args []string, env map[string]string, directory string) (windows.ProcessInformation, error) {
	var child windows.ProcessInformation
	attrs, err := windows.NewProcThreadAttributeList(1)
	if err != nil {
		return child, err
	}
	defer attrs.Delete()
	const jobList = 0x0002000d // PROC_THREAD_ATTRIBUTE_JOB_LIST (Windows 10+)
	if err := attrs.Update(jobList, unsafe.Pointer(&job), unsafe.Sizeof(job)); err != nil {
		return child, err
	}
	startup := windows.StartupInfoEx{ProcThreadAttributeList: attrs.List()}
	startup.Cb = uint32(unsafe.Sizeof(startup))
	app, err := windows.UTF16PtrFromString(executable)
	if err != nil {
		return child, err
	}
	command, err := windows.UTF16PtrFromString(windows.ComposeCommandLine(append([]string{executable}, args...)))
	if err != nil {
		return child, err
	}
	cwd, err := windows.UTF16PtrFromString(directory)
	if err != nil {
		return child, err
	}
	block := utf16.Encode([]rune(environmentBlock(env)))
	err = windows.CreateProcess(app, command, nil, nil, false, windows.EXTENDED_STARTUPINFO_PRESENT|windows.CREATE_UNICODE_ENVIRONMENT|windows.CREATE_SUSPENDED|windows.CREATE_NO_WINDOW, &block[0], cwd, &startup.StartupInfo, &child)
	runtime.KeepAlive(block)
	runtime.KeepAlive(job)
	if err != nil {
		return child, err
	}
	if _, err := windows.ResumeThread(child.Thread); err != nil {
		windows.TerminateProcess(child.Process, 1)
		windows.CloseHandle(child.Thread)
		windows.CloseHandle(child.Process)
		return windows.ProcessInformation{}, err
	}
	windows.CloseHandle(child.Thread)
	child.Thread = 0
	return child, nil
}

func alive(child windows.ProcessInformation) bool {
	status, err := windows.WaitForSingleObject(child.Process, 0)
	return err == nil && status == uint32(windows.WAIT_TIMEOUT)
}

func normalUser() (string, error) {
	token := windows.GetCurrentProcessToken()
	var elevated, size uint32
	err := windows.GetTokenInformation(token, windows.TokenElevation, (*byte)(unsafe.Pointer(&elevated)), uint32(unsafe.Sizeof(elevated)), &size)
	if err != nil || elevated != 0 {
		return "", fmt.Errorf("run Visor as your normal user without elevation")
	}
	user, err := token.GetTokenUser()
	if err != nil {
		return "", err
	}
	return user.User.Sid.String(), nil
}
