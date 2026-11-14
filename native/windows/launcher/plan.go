package main

import (
	"fmt"
	"sort"
	"strings"
)

type launchPlan struct {
	config, journal, instance, pipe string
	app, conductor                  map[string]string
}

func drivePath(value string) bool {
	return len(value) >= 3 && ((value[0] >= 'A' && value[0] <= 'Z') || (value[0] >= 'a' && value[0] <= 'z')) && value[1] == ':' && (value[2] == '\\' || value[2] == '/') && !strings.ContainsAny(value, "\x00\r\n")
}

func plan(environment map[string]string, sid, nonce string) (launchPlan, error) {
	p := launchPlan{app: map[string]string{}, conductor: map[string]string{}}
	env := map[string]string{}
	for key, value := range environment {
		env[strings.ToUpper(key)] = value
	}
	p.config, p.journal = env["HYPERMESH_CONDUCTOR_CONFIG"], env["HYPERMESH_CONDUCTOR_JOURNAL"]
	if !drivePath(p.config) || !drivePath(p.journal) {
		return p, fmt.Errorf("set absolute local HYPERMESH_CONDUCTOR_CONFIG and HYPERMESH_CONDUCTOR_JOURNAL paths")
	}
	p.instance = env["HYPERMESH_VISOR_INSTANCE"]
	if p.instance == "" {
		p.instance = "default"
	}
	if len(p.instance) > 64 {
		return p, fmt.Errorf("invalid Visor instance")
	}
	for _, c := range p.instance {
		if !(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '-') {
			return p, fmt.Errorf("invalid Visor instance")
		}
	}
	p.pipe = `\\.\pipe\hypermesh-conductor-` + sid + "-visor-" + nonce
	for _, key := range []string{"SYSTEMROOT", "WINDIR", "SYSTEMDRIVE", "USERPROFILE", "LOCALAPPDATA", "APPDATA", "TEMP", "TMP", "USERNAME", "USERDOMAIN", "HYPERMESH_DEFAULT_MODEL", "HYPERMESH_CONDUCTOR_LEASE_ID", "HYPERMESH_CONFIG_DIR", "HYPERMESH_RUNTIME_DIR", "HYPERMESH_MCP_PROJECT", "HYPERMESH_BOT_CONFIG", "HYPERMESH_COMPANION_POLL_MS", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_RUNTIME_DIR"} {
		if value, ok := env[key]; ok {
			p.app[key] = value
		}
	}
	// Fixed system path; app loading cannot inherit arbitrary DLL/profiler paths.
	p.app["PATH"] = env["SYSTEMROOT"] + `\System32;` + env["SYSTEMROOT"]
	p.app["HYPERMESH_VISOR_INSTANCE"] = p.instance
	p.app["HYPERMESH_CONDUCTOR_SOCKET"] = p.pipe
	for key, value := range env {
		if strings.HasPrefix(key, "DOTNET_") || strings.HasPrefix(key, "CORECLR_") || strings.HasPrefix(key, "COR_") || strings.HasPrefix(key, "LD_") || strings.HasPrefix(key, "DYLD_") {
			continue
		}
		p.conductor[key] = value
	}
	p.conductor["HYPERMESH_CONDUCTOR_SOCKET"] = p.pipe
	return p, nil
}

func environmentBlock(env map[string]string) string {
	keys := make([]string, 0, len(env))
	for key := range env {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	var result strings.Builder
	for _, key := range keys {
		result.WriteString(key + "=" + env[key] + "\x00")
	}
	result.WriteByte(0)
	return result.String()
}
