//go:build windows

package api

import (
	"context"
	"fmt"
	"net"
	"net/http"
	"os"
	"runtime"
	"strings"
	"time"
	"unsafe"

	"github.com/Microsoft/go-winio"
	"golang.org/x/sys/windows"
)

func userSID() (*windows.SID, error) {
	user, err := windows.GetCurrentProcessToken().GetTokenUser()
	if err != nil {
		return nil, err
	}
	return user.User.Sid.Copy()
}

func pipePrefix() (string, error) {
	sid, err := userSID()
	if err != nil {
		return "", err
	}
	return `\\.\pipe\hypermesh-visor-` + sid.String() + "-", nil
}

func validInstance(instance string) bool {
	if len(instance) == 0 || len(instance) > 64 {
		return false
	}
	for _, ch := range instance {
		if !(ch >= 'a' && ch <= 'z' || ch >= 'A' && ch <= 'Z' || ch >= '0' && ch <= '9' || ch == '-') {
			return false
		}
	}
	return true
}

func validatePipePath(path string) error {
	prefix, err := pipePrefix()
	if err != nil {
		return err
	}
	if !strings.HasPrefix(path, prefix) || !strings.HasSuffix(path, "-control") || !validInstance(strings.TrimSuffix(strings.TrimPrefix(path, prefix), "-control")) {
		return fmt.Errorf("Visor pipe must be a local current-user instance")
	}
	return nil
}

// ControlSocket selects the current user's private control pipe.
func ControlSocket() (string, error) {
	instance := os.Getenv("HYPERMESH_VISOR_INSTANCE")
	if instance == "" {
		instance = "default"
	}
	if !validInstance(instance) {
		return "", fmt.Errorf("invalid Visor pipe instance")
	}
	prefix, err := pipePrefix()
	return prefix + instance + "-control", err
}

// Native Windows has no TCP token file or TCP fallback.
func VisorToken() string { return strings.TrimSpace(os.Getenv("HYPERMESH_VISOR_TOKEN")) }

func localClient(path string) *http.Client {
	return &http.Client{Timeout: 30 * time.Second, Transport: &http.Transport{
		DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) { return dialPipe(ctx, path) },
	}}
}

func dialPipe(ctx context.Context, path string) (net.Conn, error) {
	if err := validatePipePath(path); err != nil {
		return nil, err
	}
	ctx, cancel := context.WithTimeout(ctx, 2*time.Second)
	defer cancel()
	// go-winio opens with SECURITY_SQOS_PRESENT | SECURITY_ANONYMOUS, so
	// an untrusted server cannot impersonate this process during authentication.
	conn, err := winio.DialPipeContext(ctx, path)
	if err != nil {
		return nil, err
	}
	if err := privatePipe(conn); err != nil {
		conn.Close()
		return nil, err
	}
	return conn, nil
}

func privatePipe(conn net.Conn) error {
	raw, ok := conn.(interface{ Fd() uintptr })
	if !ok {
		return fmt.Errorf("pipe handle unavailable")
	}
	sd, err := windows.GetSecurityInfo(windows.Handle(raw.Fd()), windows.SE_KERNEL_OBJECT, windows.OWNER_SECURITY_INFORMATION|windows.DACL_SECURITY_INFORMATION)
	runtime.KeepAlive(conn)
	if err != nil {
		return err
	}
	if err := privateDescriptor(sd); err != nil {
		return err
	}
	return privateServer(windows.Handle(raw.Fd()))
}

func privateDescriptor(sd *windows.SECURITY_DESCRIPTOR) error {
	current, err := userSID()
	if err != nil {
		return err
	}
	owner, _, err := sd.Owner()
	if err != nil || owner == nil || !owner.Equals(current) {
		return fmt.Errorf("Visor pipe belongs to another user")
	}
	dacl, _, err := sd.DACL()
	if err != nil || dacl == nil {
		return fmt.Errorf("Visor pipe has no restrictive DACL")
	}
	for i := uint32(0); i < uint32(dacl.AceCount); i++ {
		var ace *windows.ACCESS_ALLOWED_ACE
		if err := windows.GetAce(dacl, i, &ace); err != nil {
			return err
		}
		// Reject unfamiliar ACE forms, including object and callback grants.
		if ace.Header.AceType != windows.ACCESS_ALLOWED_ACE_TYPE || ace.Header.AceSize < 16 {
			return fmt.Errorf("unsupported Visor pipe ACE")
		}
		sid := (*windows.SID)(unsafe.Pointer(&ace.SidStart))
		if !sid.IsValid() || sid.Len() > int(ace.Header.AceSize)-8 {
			return fmt.Errorf("invalid Visor pipe trustee")
		}
		if !sid.Equals(current) && !sid.IsWellKnown(windows.WinLocalSystemSid) && !sid.IsWellKnown(windows.WinBuiltinAdministratorsSid) {
			return fmt.Errorf("Visor pipe grants another user access")
		}
	}
	runtime.KeepAlive(sd)
	return nil
}

// Same SID is not enough: sandboxed low-integrity processes can create pipes.
func privateServer(pipe windows.Handle) error {
	var pid uint32
	if err := windows.GetNamedPipeServerProcessId(pipe, &pid); err != nil {
		return err
	}
	process, err := windows.OpenProcess(windows.PROCESS_QUERY_LIMITED_INFORMATION|windows.SYNCHRONIZE, false, pid)
	if err != nil {
		return err
	}
	defer windows.CloseHandle(process)
	var token windows.Token
	if err := windows.OpenProcessToken(process, windows.TOKEN_QUERY, &token); err != nil {
		return err
	}
	defer token.Close()
	if err := trustedServerToken(token); err != nil {
		return err
	}
	var checkedPID uint32
	if err := windows.GetNamedPipeServerProcessId(pipe, &checkedPID); err != nil {
		return err
	}
	status, err := windows.WaitForSingleObject(process, 0)
	if err != nil || status != uint32(windows.WAIT_TIMEOUT) || checkedPID != pid {
		return fmt.Errorf("Visor pipe server changed or exited")
	}
	return nil
}

func trustedServerToken(token windows.Token) error {
	current, err := userSID()
	if err != nil {
		return err
	}
	user, err := token.GetTokenUser()
	if err != nil {
		return err
	}
	if !user.User.Sid.Equals(current) {
		return fmt.Errorf("Visor server belongs to another user")
	}
	restricted, err := token.IsRestricted()
	if err != nil {
		return err
	}
	if restricted {
		return fmt.Errorf("Visor server has a restricted token")
	}
	serverLevel, err := integrityLevel(token)
	if err != nil {
		return err
	}
	clientLevel, err := integrityLevel(windows.GetCurrentProcessToken())
	if err != nil {
		return err
	}
	if serverLevel < clientLevel || serverLevel < 0x2000 {
		return fmt.Errorf("Visor server has lower integrity")
	}
	return nil
}

func integrityLevel(token windows.Token) (uint32, error) {
	var size uint32
	err := windows.GetTokenInformation(token, windows.TokenIntegrityLevel, nil, 0, &size)
	if err != windows.ERROR_INSUFFICIENT_BUFFER || size < uint32(unsafe.Sizeof(windows.Tokenmandatorylabel{})) || size > 65536 {
		return 0, fmt.Errorf("cannot inspect process integrity")
	}
	buffer := make([]byte, size)
	if err := windows.GetTokenInformation(token, windows.TokenIntegrityLevel, &buffer[0], size, &size); err != nil {
		return 0, err
	}
	label := (*windows.Tokenmandatorylabel)(unsafe.Pointer(&buffer[0]))
	sid := label.Label.Sid
	if sid == nil || !sid.IsValid() || sid.SubAuthorityCount() != 1 || !strings.HasPrefix(sid.String(), "S-1-16-") {
		return 0, fmt.Errorf("invalid process integrity label")
	}
	level := sid.SubAuthority(0)
	runtime.KeepAlive(buffer)
	return level, nil
}
