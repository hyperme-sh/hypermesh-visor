// Staged under the bundled CLI module by the disposable Windows CI contract.
package main

import (
	"fmt"
	"os"
	"runtime"
	"strings"

	"github.com/zalando/go-keyring"
)

func main() {
	if runtime.GOOS != "windows" || os.Getenv("GITHUB_ACTIONS") != "true" || os.Getenv("RUNNER_ENVIRONMENT") != "github-hosted" || os.Getenv("RUNNER_OS") != "Windows" || os.Getenv("HYPERMESH_CREDENTIAL_CONTRACT") != "1" || len(os.Args) != 3 || !strings.HasPrefix(os.Args[2], "hypermesh-ci-") {
		panic("disposable Windows credential contract required")
	}
	service, account := os.Args[2], "fixture-session"
	var err error
	switch os.Args[1] {
	case "write":
		err = keyring.Set(service, account, "go synthetic λ")
	case "read-rust":
		var token string
		token, err = keyring.Get(service, account)
		if token != "rust synthetic λ" {
			panic("cross-language token mismatch")
		}
	case "delete":
		err = keyring.Delete(service, account)
	default:
		panic("unknown contract operation")
	}
	if err != nil {
		panic("cross-language credential operation failed")
	}
	fmt.Println("PASS Windows shared credential operation")
}
