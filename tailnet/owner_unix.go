//go:build unix

package main

import (
	"fmt"
	"io/fs"
	"os"
	"syscall"
)

// checkOwner refuses a file or directory that the current user does not own.
func checkOwner(fi fs.FileInfo) error {
	st, ok := fi.Sys().(*syscall.Stat_t)
	if !ok {
		return fmt.Errorf("%s: cannot read its owner", fi.Name())
	}
	if int(st.Uid) != os.Getuid() {
		return fmt.Errorf("%s belongs to another user", fi.Name())
	}
	return nil
}
