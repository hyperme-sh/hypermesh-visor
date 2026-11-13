//go:build windows

package api

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/Microsoft/go-winio"
	"golang.org/x/sys/windows"
)

func testPipePath(t *testing.T) string {
	t.Helper()
	t.Setenv("HYPERMESH_VISOR_INSTANCE", fmt.Sprintf("unit-%d-%d", os.Getpid(), time.Now().UnixNano()))
	path, err := ControlSocket()
	if err != nil {
		t.Fatal(err)
	}
	return path
}

func TestWindowsVisorPromptPrivatePipe(t *testing.T) {
	path := testPipePath(t)
	sid, err := userSID()
	if err != nil {
		t.Fatal(err)
	}
	listener, err := winio.ListenPipe(path, &winio.PipeConfig{SecurityDescriptor: "O:" + sid.String() + "D:P(A;;GA;;;" + sid.String() + ")(A;;GA;;;SY)"})
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	requests := make(chan string, 1)
	server := &http.Server{Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		data, _ := io.ReadAll(r.Body)
		if r.Method != "POST" || !strings.HasSuffix(r.URL.Path, "/stream") || r.Header.Get(HeaderAPIKey) != "org_fixture" || r.Header.Get("Authorization") != "" {
			requests <- "bad request"
		} else {
			requests <- string(data)
		}
		w.Write([]byte("{\"kind\":\"done\"}\n"))
	})}
	defer server.Close()
	go server.Serve(listener)
	client := NewClient("", "", "org_fixture", "")
	body, err := client.PostVisorPrompt("", "12345678-1234-1234-1234-123456789abc", "hello pipe", "")
	if err != nil || !strings.Contains(string(body), "done") {
		t.Fatal(string(body), err)
	}
	if request := <-requests; !strings.Contains(request, "hello pipe") {
		t.Fatal(request)
	}
}

func TestWindowsVisorPathAndCancellation(t *testing.T) {
	path := testPipePath(t)
	for _, invalid := range []string{`\\remote\pipe\anything`, `C:\pipe`, path + `\child`, strings.TrimSuffix(path, "-control") + "-companion"} {
		if _, err := dialPipe(context.Background(), invalid); err == nil {
			t.Fatal("accepted invalid endpoint")
		}
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := dialPipe(ctx, path); err == nil {
		t.Fatal("ignored cancellation")
	}
	t.Setenv("HYPERMESH_VISOR_INSTANCE", "../invalid")
	if _, err := ControlSocket(); err == nil {
		t.Fatal("accepted invalid instance")
	}
	t.Setenv("HYPERMESH_VISOR_TOKEN", " explicit ")
	if VisorToken() != "explicit" {
		t.Fatal("token override")
	}
}

func TestPipeDescriptorRefusesBroadOrForeignAccess(t *testing.T) {
	user, err := userSID()
	if err != nil {
		t.Fatal(err)
	}
	owner := "O:" + user.String()
	for _, descriptor := range []string{
		owner + "D:(A;;GA;;;WD)", owner + "D:NO_ACCESS_CONTROL",
		"O:SYD:(A;;GA;;;" + user.String() + ")",
		owner + "D:(D;;GA;;;WD)(A;;GA;;;" + user.String() + ")",
	} {
		sd, err := windows.SecurityDescriptorFromString(descriptor)
		if err != nil {
			t.Fatal(err)
		}
		if err := privateDescriptor(sd); err == nil {
			t.Fatalf("accepted %s", descriptor)
		}
	}
}

func TestPipeRejectsDefaultACLBeforeRequest(t *testing.T) {
	path := testPipePath(t)
	listener, err := winio.ListenPipe(path, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	done := make(chan error, 1)
	go func() {
		conn, err := listener.Accept()
		if err != nil {
			done <- err
			return
		}
		defer conn.Close()
		conn.SetReadDeadline(time.Now().Add(5 * time.Second))
		data := make([]byte, 1)
		n, err := conn.Read(data)
		if n != 0 {
			done <- fmt.Errorf("sent bytes before authentication")
			return
		}
		if err != io.EOF {
			done <- fmt.Errorf("expected closed connection, got %v", err)
			return
		}
		done <- nil
	}()
	if conn, err := dialPipe(context.Background(), path); err == nil {
		conn.Close()
		t.Fatal("accepted default ACL")
	}
	if err := <-done; err != nil {
		t.Fatal(err)
	}
}
