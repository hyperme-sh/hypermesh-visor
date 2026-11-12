// Staged below the vendored CLI module only by the native CI contract runner.
package main

import (
	"fmt"
	"os"

	"github.com/FyberLabs/hypermesh-cli/internal/session"
)

func main() {
	if os.Getenv("VISOR_ISOLATED_KEYCHAIN_CONTRACT") != "1" {
		panic("isolated keychain contract runner required")
	}
	store := session.KeyringStore{}
	require := func(ok bool) {
		if !ok {
			panic("native credential store contract failed")
		}
	}
	token, err := store.Refresh()
	require(err == nil && token == "")
	require(store.PutRefresh("") != nil)
	for _, fixture := range []string{"fixture-first", "fixture replacement\nUnicode: λ; 'quoted'"} {
		require(store.PutRefresh(fixture) == nil)
		token, err = store.Refresh()
		require(err == nil && token == fixture)
	}
	require(store.Delete() == nil)
	token, err = store.Refresh()
	require(err == nil && token == "")
	require(store.Delete() == nil)
	fmt.Println("PASS native CLI keychain write, replace, read, empty refusal and idempotent delete")
}
