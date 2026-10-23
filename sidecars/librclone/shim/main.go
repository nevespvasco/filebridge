// filebridge's librclone (ADR 0014): rclone's own C shims (librclone/librclone.go, MIT) with
// only the backends filebridge's Transport uses, so the library stays small on phones.
//
// build.sh copies this file into the rclone source tree (it needs rclone's go.mod) and builds it
// with -buildmode=c-shared (Android) or c-archive (iOS).
package main

/*
#include <stdlib.h>

struct RcloneRPCResult {
	char*	Output;
	int	Status;
};
*/
import "C"

import (
	"os"
	"unsafe"

	"github.com/rclone/rclone/filebridge/quietconfig"
	"github.com/rclone/rclone/librclone/librclone"

	// Backends a Destination can name (transport-api `Backend`).
	_ "github.com/rclone/rclone/backend/local"
	_ "github.com/rclone/rclone/backend/sftp"
	_ "github.com/rclone/rclone/backend/webdav"

	_ "github.com/rclone/rclone/fs/operations" // operations/* rc calls
	_ "github.com/rclone/rclone/fs/sync"       // sync/* rc calls
)

// quietconfig set RCLONE_CONFIG only so that fs/config's init stays quiet; nothing after it
// should see it.
func init() {
	if quietconfig.Set {
		_ = os.Unsetenv("RCLONE_CONFIG")
	}
}

// RcloneSetenv sets a variable in Go's own copy of the environment. A library loaded with
// dlopen only sees the environment the process started with, and Android starts apps without
// HOME or TMPDIR, so the host passes them here before RcloneInitialize.
//
//export RcloneSetenv
func RcloneSetenv(key *C.char, value *C.char) {
	_ = os.Setenv(C.GoString(key), C.GoString(value))
}

//export RcloneInitialize
func RcloneInitialize() {
	librclone.Initialize()
}

//export RcloneFinalize
func RcloneFinalize() {
	librclone.Finalize()
}

// RcloneRPC calls an rc method with JSON input; the caller frees Output with RcloneFreeString.
//
//export RcloneRPC
func RcloneRPC(method *C.char, input *C.char) (result C.struct_RcloneRPCResult) { //nolint:golint
	output, status := librclone.RPC(C.GoString(method), C.GoString(input))
	result.Output = C.CString(output)
	result.Status = C.int(status)
	return result
}

//export RcloneFreeString
func RcloneFreeString(str *C.char) {
	C.free(unsafe.Pointer(str))
}

func main() {}
