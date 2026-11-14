package main

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"runtime"
	"strings"
	"time"
	"unsafe"

	"github.com/Microsoft/go-winio"
	"golang.org/x/sys/windows"
)

func currentEnvironment() map[string]string {
	env := map[string]string{}
	for _, entry := range os.Environ() {
		if key, value, ok := strings.Cut(entry, "="); ok && key != "" {
			env[key] = value
		}
	}
	return env
}

func instanceLock(sid, instance string) (windows.Handle, error) {
	sd, err := windows.SecurityDescriptorFromString("O:" + sid + "D:P(A;;GA;;;" + sid + ")(A;;GA;;;SY)")
	if err != nil {
		return 0, err
	}
	attributes := windows.SecurityAttributes{SecurityDescriptor: sd}
	attributes.Length = uint32(unsafe.Sizeof(attributes))
	name, err := windows.UTF16PtrFromString(`Local\HypermeshVisor-` + sid + "-" + instance)
	if err != nil {
		return 0, err
	}
	mutex, err := windows.CreateMutex(&attributes, false, name)
	runtime.KeepAlive(sd)
	if err != nil {
		if mutex != 0 {
			windows.CloseHandle(mutex)
		}
		return 0, fmt.Errorf("Visor is already running or its instance lock is unavailable")
	}
	status, err := windows.WaitForSingleObject(mutex, 0)
	if err != nil || (status != windows.WAIT_OBJECT_0 && status != windows.WAIT_ABANDONED) {
		windows.CloseHandle(mutex)
		return 0, fmt.Errorf("Visor is already running")
	}
	return mutex, nil
}

func ready(ctx context.Context, pipe string, child windows.ProcessInformation) bool {
	transport := &http.Transport{DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
		conn, err := winio.DialPipeContext(ctx, pipe)
		if err != nil {
			return nil, err
		}
		raw, ok := conn.(interface{ Fd() uintptr })
		var pid uint32
		if !ok {
			conn.Close()
			return nil, fmt.Errorf("pipe handle unavailable")
		}
		err = windows.GetNamedPipeServerProcessId(windows.Handle(raw.Fd()), &pid)
		runtime.KeepAlive(conn)
		if err != nil || pid != child.ProcessId || !alive(child) {
			conn.Close()
			return nil, fmt.Errorf("Conductor identity mismatch")
		}
		return conn, nil
	}}
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, Timeout: 500 * time.Millisecond, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	req, err := http.NewRequestWithContext(ctx, "GET", "http://localhost/v1/models", nil)
	if err != nil {
		return false
	}
	response, err := client.Do(req)
	if err != nil {
		return false
	}
	defer response.Body.Close()
	_, err = io.Copy(io.Discard, io.LimitReader(response.Body, 65537))
	return err == nil && response.StatusCode == http.StatusOK
}

func run() error {
	if len(os.Args) != 1 {
		return fmt.Errorf("configure HYPERMESH_CONDUCTOR_CONFIG and HYPERMESH_CONDUCTOR_JOURNAL, then run this launcher without arguments")
	}
	sid, err := normalUser()
	if err != nil {
		return err
	}
	var random [12]byte
	if _, err := rand.Read(random[:]); err != nil {
		return err
	}
	p, err := plan(currentEnvironment(), sid, hex.EncodeToString(random[:]))
	if err != nil {
		return err
	}
	// Win32 mutex ownership is thread-affine.
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	mutex, err := instanceLock(sid, p.instance)
	if err != nil {
		return err
	}
	defer windows.CloseHandle(mutex)
	defer windows.ReleaseMutex(mutex)
	executable, err := os.Executable()
	if err != nil {
		return err
	}
	directory := filepath.Dir(executable)
	for _, name := range []string{"conductor.exe", "hypermesh-native-desktop.exe"} {
		info, err := os.Stat(filepath.Join(directory, name))
		if err != nil || !info.Mode().IsRegular() {
			return fmt.Errorf("install the complete Visor development bundle")
		}
	}
	job, err := newJob()
	if err != nil {
		return fmt.Errorf("process containment unavailable")
	}
	defer windows.CloseHandle(job)
	conductor, err := spawn(job, filepath.Join(directory, "conductor.exe"), []string{"serve", "--socket", "--config", p.config, "--journal", p.journal}, p.conductor, directory)
	if err != nil {
		return fmt.Errorf("Conductor could not start")
	}
	defer windows.CloseHandle(conductor.Process)
	stop := make(chan os.Signal, 1)
	signal.Notify(stop, os.Interrupt)
	defer signal.Stop(stop)
	deadline := time.Now().Add(10 * time.Second)
	for {
		if !alive(conductor) || time.Now().After(deadline) {
			return fmt.Errorf("Conductor did not become ready; check its endpoint configuration")
		}
		ctx, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
		ok := ready(ctx, p.pipe, conductor)
		cancel()
		if ok {
			break
		}
		select {
		case <-stop:
			return nil
		case <-time.After(100 * time.Millisecond):
		}
	}
	app, err := spawn(job, filepath.Join(directory, "hypermesh-native-desktop.exe"), nil, p.app, directory)
	if err != nil {
		return fmt.Errorf("desktop companion could not start")
	}
	defer windows.CloseHandle(app.Process)
	for {
		if !alive(app) {
			return nil
		}
		if !alive(conductor) {
			return fmt.Errorf("Conductor stopped; the desktop companion has been stopped")
		}
		select {
		case <-stop:
			return nil
		case <-time.After(100 * time.Millisecond):
		}
	}
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		message, _ := windows.UTF16PtrFromString(err.Error())
		title, _ := windows.UTF16PtrFromString("Hypermesh Visor")
		windows.NewLazySystemDLL("user32.dll").NewProc("MessageBoxW").Call(0, uintptr(unsafe.Pointer(message)), uintptr(unsafe.Pointer(title)), 0x10)
		runtime.KeepAlive(message)
		runtime.KeepAlive(title)
		os.Exit(1)
	}
}
