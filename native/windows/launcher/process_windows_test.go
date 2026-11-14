package main

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"golang.org/x/sys/windows"
)

func TestChildFixture(t *testing.T) {
	if os.Getenv("VISOR_JOB_FIXTURE") != "1" {
		return
	}
	time.Sleep(30 * time.Second)
	os.Exit(0)
}

func TestJobOwnsChildBeforeExecutionAndKillsOnClose(t *testing.T) {
	job, err := newJob()
	if err != nil {
		t.Fatal(err)
	}
	defer func() {
		if job != 0 {
			windows.CloseHandle(job)
		}
	}()
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	env := currentEnvironment()
	env["VISOR_JOB_FIXTURE"] = "1"
	child, err := spawn(job, executable, []string{"-test.run=^TestChildFixture$"}, env, filepath.Dir(executable))
	if err != nil {
		t.Fatal(err)
	}
	defer windows.CloseHandle(child.Process)
	if !alive(child) {
		t.Fatal("fixture did not start")
	}
	if err := windows.CloseHandle(job); err != nil {
		t.Fatal(err)
	}
	job = 0
	status, err := windows.WaitForSingleObject(child.Process, 5000)
	if err != nil || status != windows.WAIT_OBJECT_0 {
		t.Fatal("child survived launcher job close", status, err)
	}
}
