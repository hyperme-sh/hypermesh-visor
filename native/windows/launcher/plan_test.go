package main

import (
	"strings"
	"testing"
)

func TestPlanKeepsServiceCredentialsOutOfDesktop(t *testing.T) {
	env := map[string]string{"HYPERMESH_CONDUCTOR_CONFIG": `C:\config with quotes'\endpoints.json`, "HYPERMESH_CONDUCTOR_JOURNAL": `C:\data\journal.jsonl`, "SystemRoot": `C:\Windows`, "CONDUCTOR_AIUDITOR_TOKEN": "secret", "HYPERMESH_CONDUCTOR_SOCKET": "old", "DOTNET_STARTUP_HOOKS": "inject", "PATH": "unsafe", "HYPERMESH_DEFAULT_MODEL": "model"}
	p, err := plan(env, "S-1-5-21-1", "abc")
	if err != nil {
		t.Fatal(err)
	}
	if p.app["CONDUCTOR_AIUDITOR_TOKEN"] != "" || p.app["HYPERMESH_CONDUCTOR_CONFIG"] != "" || p.app["DOTNET_STARTUP_HOOKS"] != "" || p.app["PATH"] == "unsafe" {
		t.Fatal("desktop inherited service or loading configuration")
	}
	if p.conductor["CONDUCTOR_AIUDITOR_TOKEN"] != "secret" || p.conductor["DOTNET_STARTUP_HOOKS"] != "" || p.app["HYPERMESH_DEFAULT_MODEL"] != "model" {
		t.Fatal("environment split")
	}
	if p.app["HYPERMESH_CONDUCTOR_SOCKET"] != p.conductor["HYPERMESH_CONDUCTOR_SOCKET"] || p.pipe != `\\.\pipe\hypermesh-conductor-S-1-5-21-1-visor-abc` {
		t.Fatal("pipe selection")
	}
	if p.instance != "default" || p.config != env["HYPERMESH_CONDUCTOR_CONFIG"] {
		t.Fatal("plan changed explicit configuration")
	}
	for _, bad := range []string{"relative", `C:relative`, `\\server\share`, "C:\\bad\x00path"} {
		env["HYPERMESH_CONDUCTOR_CONFIG"] = bad
		if _, err := plan(env, "sid", "nonce"); err == nil {
			t.Fatal("accepted nonlocal or relative path")
		}
	}
	if environmentBlock(map[string]string{"Z": "last", "A": "first"}) != "A=first\x00Z=last\x00\x00" {
		t.Fatal("Windows environment block")
	}
	for _, bad := range []string{"../bad", strings.Repeat("a", 65), "space name"} {
		env["HYPERMESH_CONDUCTOR_CONFIG"] = `C:\config`
		env["HYPERMESH_VISOR_INSTANCE"] = bad
		if _, err := plan(env, "sid", "nonce"); err == nil {
			t.Fatal("accepted invalid instance")
		}
	}
}
